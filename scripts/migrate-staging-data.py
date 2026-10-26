#!/usr/bin/env python3
"""Explicitly move the formerly shared macOS profile to isolated staging storage.

Run from an authorised Terminal, with every Minutus instance stopped. This is a
local maintenance tool, never a release/staging startup hook. Vaults stay opaque;
external weights are neither followed nor moved. Only profile-owned roots move.
"""

import argparse
from contextlib import closing
import datetime
import json
import os
from pathlib import Path
import sqlite3
import stat
import subprocess
import sys

PRODUCTION_ID = "com.omarperacha.zeta"
STAGING_ID = PRODUCTION_ID + ".staging"


def checked_directory(path):
    """Reject indirect roots/ancestors, including dangling symlinks."""
    for parent in reversed((path, *path.parents)):
        try:
            metadata = parent.lstat()
        except FileNotFoundError:
            return False
        if not stat.S_ISDIR(metadata.st_mode):
            raise ValueError(f"Expected a real directory, without symlink ancestors: {parent}")
    return True


def profile_pairs(home):
    support = home / "Library/Application Support"
    containers = home / "Library/Containers"
    return [
        (containers / PRODUCTION_ID / "Data/Library/Application Support" / PRODUCTION_ID,
         containers / STAGING_ID / "Data/Library/Application Support" / STAGING_ID),
        (support / PRODUCTION_ID, support / STAGING_ID),
    ]


def plan(home, archive_existing=False):
    pairs = []
    for source, destination in profile_pairs(home):
        if not checked_directory(source):
            continue
        if not checked_directory(destination.parent):
            raise ValueError(
                "Build and open Minutus Staging once, then quit it, to let macOS "
                "initialise its container before migration."
            )
        exists = checked_directory(destination)
        if exists and not archive_existing:
            raise ValueError(
                "A staging profile already exists. Use --archive-existing-staging "
                "to preserve it separately before moving the old profile."
            )
        if source.stat().st_dev != destination.parent.stat().st_dev:
            raise ValueError("Migration requires a same-filesystem rename; copying is forbidden.")
        database = source / "zeta.sqlite3"
        if not database.is_file() or database.is_symlink():
            raise ValueError(f"Expected a regular application database in {source}")
        pairs.append((source, destination, exists))
    if not pairs:
        raise ValueError("No old shared profiles remain to migrate.")
    return pairs


def relocated(value, source, destination):
    prefix = str(source) + "/"
    return str(destination) + value[len(str(source)):] if value.startswith(prefix) else value


def rebase_database(database, source, destination):
    """Rebase only known model-location fields; never rewrite chats or secrets."""
    with closing(sqlite3.connect(database)) as connection, connection:
        connection.execute("BEGIN IMMEDIATE")
        tables = {row[0] for row in connection.execute("SELECT name FROM sqlite_master WHERE type='table'")}
        for table in ("models", "model_companions"):
            if table not in tables:
                continue
            for rowid, path in connection.execute(f"SELECT rowid, local_path FROM {table}").fetchall():
                replacement = relocated(path, source, destination)
                if replacement != path:
                    connection.execute(f"UPDATE {table} SET local_path=? WHERE rowid=?", (replacement, rowid))
        if "external_model_registrations" in tables:
            for model_id, raw in connection.execute("SELECT model_id, record FROM external_model_registrations").fetchall():
                record = json.loads(raw)
                changed = False
                for file in record["files"]:
                    replacement = relocated(file["path"], source, destination)
                    changed |= replacement != file["path"]
                    file["path"] = replacement
                if changed:
                    connection.execute("UPDATE external_model_registrations SET record=? WHERE model_id=?",
                                       (json.dumps(record), model_id))


def rebase_aliases(root, source, destination):
    """Change only app-owned Ollama aliases into the moved profile, not targets."""
    blobs = root / "ollama-models/blobs"
    if not checked_directory(blobs):
        return
    for entry in blobs.iterdir():
        if not entry.is_symlink():
            continue
        original = os.readlink(entry)
        replacement = relocated(original, source, destination)
        if replacement != original:
            temporary = entry.with_name(entry.name + ".migration")
            os.symlink(replacement, temporary)
            os.replace(temporary, entry)


def migrate(pairs, receipt):
    """Retain a private database snapshot and progress record before each rename.

    On failure stop with the receipt intact: no automatic reverse move of a
    partially rebased profile. The receipt identifies the last completed step.
    """
    receipt.mkdir(mode=0o700, parents=False, exist_ok=False)
    record = {"profiles": []}
    journal = receipt / "migration.json"

    def save():
        temporary = receipt / "migration.json.tmp"
        temporary.write_text(json.dumps(record, indent=2) + "\n")
        temporary.chmod(0o600)
        os.replace(temporary, journal)

    for index, (source, destination, exists) in enumerate(pairs):
        # backup() includes committed WAL contents without inspecting wallet keys.
        snapshot = receipt / f"profile-{index}.sqlite3"
        snapshot.touch(mode=0o600, exist_ok=False)
        with closing(sqlite3.connect((source / "zeta.sqlite3").as_uri() + "?mode=ro", uri=True)) as database, closing(sqlite3.connect(snapshot)) as backup:
            database.backup(backup)
        state = {"source": str(source), "destination": str(destination), "phase": "prepared"}
        record["profiles"].append(state)
        save()
        if exists:
            archive = destination.with_name(destination.name + ".before-migration-" + receipt.name)
            if archive.exists() or archive.is_symlink():
                raise ValueError("The staging archive path already exists.")
            state["archive"] = str(archive)
            save()
            os.rename(destination, archive)
        # Rename, never shutil.move: no hidden cross-volume copy of model weights.
        os.rename(source, destination)
        state["phase"] = "moved"
        save()
        rebase_database(destination / "zeta.sqlite3", source, destination)
        state["phase"] = "database-rebased"
        save()
        rebase_aliases(destination, source, destination)
        state["phase"] = "complete"
        save()
    return journal


def ensure_stopped():
    running = subprocess.run(
        ["pgrep", "-fl", r"zeta-desktop|Minutus[^/]*/Contents/MacOS/|/ollama-runtime/ollama"],
        capture_output=True, check=False,
    )
    if running.returncode != 1:
        raise ValueError("Quit every Minutus app, tauri:dev and their inference processes before migration (or check pgrep).")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--apply", action="store_true", help="Move the profiles; otherwise print the plan only")
    parser.add_argument("--archive-existing-staging", action="store_true", help="Preserve an existing staging profile beside its destination")
    args = parser.parse_args()
    if sys.platform != "darwin":
        parser.error("This migration is macOS-only.")
    try:
        ensure_stopped()
        pairs = plan(Path.home(), args.archive_existing_staging)
        for source, destination, exists in pairs:
            print(f"Move {source}\n  to {destination}" + ("\n  preserving the existing staging profile separately" if exists else ""))
        if not args.apply:
            print("Plan only. Rerun with --apply to move these profiles.")
            return 0
        parent = Path.home() / "Library/Application Support/Minutus Staging Migrations"
        if not checked_directory(parent):
            parent.mkdir(mode=0o700)
        receipt = parent / datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%S%fZ")
        print(f"Private migration record and database snapshots: {receipt}", flush=True)
        migrate(pairs, receipt)
        print("Migration complete. Launch Minutus Staging. Production will create a fresh profile on next launch.")
        print("Re-authorise project folders and use Locate files for external models if macOS requires it.")
        return 0
    except (OSError, ValueError, sqlite3.Error, json.JSONDecodeError) as error:
        print(f"Migration stopped: {error}", file=sys.stderr)
        print("No profiles are deleted. Inspect the migration record before retrying a partial move. macOS container access may require Terminal Full Disk Access.", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
