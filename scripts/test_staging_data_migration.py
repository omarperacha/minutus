"""Synthetic profile migrations only: never access a live wallet or user profile."""

from contextlib import closing
import importlib.util
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location("staging_data_migration", Path(__file__).with_name("migrate-staging-data.py"))
migration = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(migration)


class MigrationTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.home = Path(self.temporary.name).resolve()
        self.source, self.destination = migration.profile_pairs(self.home)[0]
        self.source.mkdir(parents=True, mode=0o700)
        self.destination.parent.mkdir(parents=True, mode=0o700)
        self.receipt = self.home / "receipt"
        self.external = self.home / "external.gguf"
        self.external.write_bytes(b"external model: never move or copy")
        self.managed = self.source / "models/installed/model.gguf"
        self.managed.parent.mkdir(parents=True)
        self.managed.write_bytes(b"managed model")
        self.vault = self.source / "app-vault/encrypted.vault"
        self.vault.parent.mkdir()
        self.vault.write_bytes(b"opaque synthetic encrypted vault")
        with closing(sqlite3.connect(self.source / "zeta.sqlite3")) as database, database:
            database.executescript('''
                CREATE TABLE models (id TEXT, local_path TEXT);
                CREATE TABLE model_companions (id TEXT, local_path TEXT);
                CREATE TABLE external_model_registrations (model_id TEXT, record TEXT);
                CREATE TABLE conversations (body TEXT);
            ''')
            for id_, path in (("managed", self.managed), ("external", self.external),
                              ("prefix-collision", Path(str(self.source) + "-other/file.gguf"))):
                database.execute("INSERT INTO models VALUES (?,?)", (id_, str(path)))
            database.execute("INSERT INTO model_companions VALUES (?,?)", ("vision", str(self.managed)))
            database.execute("INSERT INTO external_model_registrations VALUES (?,?)", ("external", json.dumps({"files": [{"path": str(self.external)}]})))
            database.execute("INSERT INTO conversations VALUES (?)", ("Keep this old path as chat text: " + str(self.source),))
        blobs = self.source / "ollama-models/blobs"
        blobs.mkdir(parents=True)
        (blobs / "managed-alias").symlink_to(self.managed)
        (blobs / "external-alias").symlink_to(self.external)

    def test_move_retains_vault_and_model_inodes_rebases_owned_paths_and_preserves_external_files(self):
        vault_inode = self.vault.stat().st_ino
        model_inode = self.managed.stat().st_ino
        external_inode = self.external.stat().st_ino
        original_chat = "Keep this old path as chat text: " + str(self.source)
        pairs = migration.plan(self.home)
        self.assertTrue(self.source.exists())  # Planning does not mutate profiles.
        journal = migration.migrate(pairs, self.receipt)
        self.assertFalse(self.source.exists())
        self.assertEqual((self.destination / "app-vault/encrypted.vault").stat().st_ino, vault_inode)
        self.assertEqual((self.destination / "models/installed/model.gguf").stat().st_ino, model_inode)
        self.assertEqual(self.external.stat().st_ino, external_inode)
        self.assertEqual(self.external.read_bytes(), b"external model: never move or copy")
        with closing(sqlite3.connect(self.destination / "zeta.sqlite3")) as database:
            paths = dict(database.execute("SELECT id, local_path FROM models"))
            self.assertEqual(paths["managed"], str(self.destination / "models/installed/model.gguf"))
            self.assertEqual(paths["external"], str(self.external))
            self.assertEqual(paths["prefix-collision"], str(self.source) + "-other/file.gguf")
            self.assertEqual(database.execute("SELECT body FROM conversations").fetchone()[0], original_chat)
        self.assertEqual(os.readlink(self.destination / "ollama-models/blobs/managed-alias"), str(self.destination / "models/installed/model.gguf"))
        self.assertEqual(os.readlink(self.destination / "ollama-models/blobs/external-alias"), str(self.external))
        self.assertEqual(json.loads(journal.read_text())["profiles"][0]["phase"], "complete")
        self.assertEqual(self.receipt.stat().st_mode & 0o777, 0o700)
        self.assertEqual((self.receipt / "profile-0.sqlite3").stat().st_mode & 0o777, 0o600)
        with closing(sqlite3.connect(self.receipt / "profile-0.sqlite3")) as database:
            self.assertEqual(dict(database.execute("SELECT id, local_path FROM models"))["managed"], str(self.managed))
        with self.assertRaisesRegex(ValueError, "No old shared profiles"):
            migration.plan(self.home)

    def test_existing_destination_requires_explicit_archive_and_is_preserved(self):
        self.destination.mkdir()
        sentinel = self.destination / "existing-vault"
        sentinel.write_bytes(b"retain")
        with self.assertRaisesRegex(ValueError, "already exists"):
            migration.plan(self.home)
        migration.migrate(migration.plan(self.home, archive_existing=True), self.receipt)
        record = json.loads((self.receipt / "migration.json").read_text())["profiles"][0]
        self.assertEqual((Path(record["archive"]) / "existing-vault").read_bytes(), b"retain")

    def test_symlink_source_or_database_is_rejected_without_moving_anything(self):
        database = self.source / "zeta.sqlite3"
        database.rename(self.source / "real.sqlite3")
        database.symlink_to("real.sqlite3")
        with self.assertRaisesRegex(ValueError, "regular application database"):
            migration.plan(self.home)
        self.assertTrue(self.source.exists())
        self.assertFalse(self.destination.exists())

    def test_mac_os_must_initialise_new_container_first(self):
        self.destination.parent.rmdir()
        with self.assertRaisesRegex(ValueError, "initialise its container"):
            migration.plan(self.home)

    def test_partial_failure_keeps_receipt_original_snapshot_and_moved_vault(self):
        with patch.object(migration, "rebase_database", side_effect=sqlite3.DatabaseError("synthetic failure")):
            with self.assertRaises(sqlite3.DatabaseError):
                migration.migrate(migration.plan(self.home), self.receipt)
        self.assertTrue((self.destination / "app-vault/encrypted.vault").exists())
        self.assertTrue((self.receipt / "profile-0.sqlite3").exists())
        record = json.loads((self.receipt / "migration.json").read_text())["profiles"][0]
        self.assertEqual(record["phase"], "moved")

    def test_committed_wal_data_is_included_in_snapshot_and_moved_database(self):
        subprocess.run([sys.executable, "-c", """
import os, sqlite3, sys
connection = sqlite3.connect(sys.argv[1])
connection.execute('PRAGMA journal_mode=WAL')
connection.execute("INSERT INTO conversations VALUES ('committed WAL record')")
connection.commit()
os._exit(0)
""", str(self.source / "zeta.sqlite3")], check=True)
        self.assertTrue((self.source / "zeta.sqlite3-wal").exists())
        migration.migrate(migration.plan(self.home), self.receipt)
        for path in (self.destination / "zeta.sqlite3", self.receipt / "profile-0.sqlite3"):
            with closing(sqlite3.connect(path)) as connection:
                self.assertEqual(connection.execute("SELECT COUNT(*) FROM conversations").fetchone()[0], 2)

    def test_running_or_failed_process_check_stops_migration(self):
        for result in (0, 2):
            with patch.object(migration.subprocess, "run") as run:
                run.return_value.returncode = result
                with self.assertRaisesRegex(ValueError, "Quit every Minutus"):
                    migration.ensure_stopped()


if __name__ == "__main__":
    unittest.main()
