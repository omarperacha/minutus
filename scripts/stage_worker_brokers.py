#!/usr/bin/env python3
"""Stage immutable role-specific XPC launchers after worker identity pinning."""
import argparse
import hashlib
from pathlib import Path
import plistlib
import re
import shutil
import subprocess
import sys
import tempfile

from stage_evm_workers import embedded_manifest, identity
from worker_broker_manifest import MAGIC, ROLES, parse

ROOT = Path(__file__).resolve().parent.parent
TARGET = "aarch64-apple-darwin"
PAYLOAD = ROOT / "src-tauri/worker-brokers"
NATIVE = ROOT / "crates/worker-transport/native"
SOURCES = ("main.c", "child.c", "profile.c", "protocol.c")


def regular(path):
    if path.is_symlink() or not path.is_file():
        raise ValueError("Indirect or missing worker broker payload")


def digest(path):
    regular(path)
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def info(role):
    return {"CFBundleExecutable": role.executable, "CFBundleIdentifier": role.service,
            "CFBundlePackageType": "XPC!", "CFBundleVersion": "1",
            "CFBundleShortVersionString": "1", "LSMinimumSystemVersion": "13.3",
            "XPCService": {"RunLoopType": "dispatch_main", "ServiceType": "Application"}}


def assert_executable(path):
    if subprocess.check_output(["/usr/bin/lipo", "-archs", str(path)], text=True).strip() != "arm64":
        raise ValueError("Worker broker must be arm64 only")
    lines = subprocess.check_output(["/usr/bin/otool", "-L", str(path)], text=True).splitlines()[1:]
    if any(not line.strip().startswith(("/usr/lib/", "/System/Library/")) for line in lines):
        raise ValueError("Worker broker has a non-system dynamic dependency")


def stage_sources():
    source = PAYLOAD / "source"
    source.mkdir(parents=True, exist_ok=True)
    expected = set()
    for path in sorted(NATIVE.rglob("*")):
        if path.is_dir():
            continue
        regular(path)
        if path.suffix not in (".c", ".h"):
            raise ValueError("Unexpected native worker transport build input")
        relative = path.relative_to(NATIVE)
        target = source / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(path, target)
        expected.add(target.relative_to(PAYLOAD).as_posix())
    shutil.copyfile(ROOT / "LICENSE", source / "LICENSE")
    (source / "README.txt").write_text(
        "Minutus fixed-role worker transport. Apache-2.0; full license included.\n"
        "The service launches and attests its one fixed contained worker, then\n"
        "seals itself before accepting any XPC message. Source/signed data use\n"
        "authenticated anonymous pipe endpoints, never named XPC messages.\n"
        "Roles, limits, profiles and worker image pins are build-owned.\n", encoding="utf-8")
    expected.update(("source/LICENSE", "source/README.txt"))
    actual = {path.relative_to(PAYLOAD).as_posix() for path in source.rglob("*") if path.is_file()}
    if actual != expected:
        raise ValueError("Stale or unexpected worker broker source notice; remove stale staging output explicitly")
    (PAYLOAD / "source.sha256").write_text("".join(
        f"{digest(PAYLOAD / name)}  {name}\n" for name in sorted(expected)), encoding="ascii")


def stage(signing_identity="-"):
    if sys.platform != "darwin" or (signing_identity != "-" and re.fullmatch(r"[0-9a-fA-F]{40}", signing_identity) is None):
        raise ValueError("Expected macOS and an exact signing identity or ad-hoc -")
    PAYLOAD.mkdir(parents=True, exist_ok=True)
    stage_sources()
    records = [MAGIC]
    for role in ROLES:
        worker = ROOT / "src-tauri/binaries" / f"{role.worker}-{TARGET}"
        worker_sha, worker_cd = identity(worker)
        regular(worker)
        service = PAYLOAD / "services" / f"{role.service}.xpc"
        macos = service / "Contents/MacOS"
        macos.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(worker, macos / role.worker)
        (macos / role.worker).chmod(0o755)
        (service / "Contents/Info.plist").write_bytes(plistlib.dumps(info(role)))
        with tempfile.TemporaryDirectory(prefix="minutus-worker-broker-") as temporary:
            header = Path(temporary) / "role.h"
            header.write_text(
                f'#define MINUTUS_WORKER_ROLE "{role.name}"\n'
                f'#define MINUTUS_WORKER_NAME "{role.worker}"\n'
                f'#define MINUTUS_WORKER_SHA256 "{worker_sha}"\n'
                f'#define MINUTUS_WORKER_CDHASH "{worker_cd}"\n'
                f'#define MINUTUS_WORKER_MEMORY {role.memory}ULL\n'
                f'#define MINUTUS_WORKER_FILE_LIMIT {role.file_limit}ULL\n'
                f'#define MINUTUS_WORKER_WALL_SECONDS {role.wall}\n', encoding="ascii")
            subprocess.run(["/usr/bin/clang", "-target", "arm64-apple-macosx13.3", "-O2", "-fblocks",
                            "-Wall", "-Wextra", "-Werror", "-include", str(header),
                            *[str(NATIVE / "broker" / name) for name in SOURCES],
                            "-o", str(macos / role.executable)], check=True)
        assert_executable(macos / role.executable)
        command = ["/usr/bin/codesign", "--force", "--sign", signing_identity,
                   "--identifier", role.service, "--options", "runtime", "--digest-algorithm=sha256"]
        if signing_identity != "-":
            command.append("--timestamp")
        subprocess.run([*command, str(service)], check=True)
        broker_sha, broker_cd = identity(macos / role.executable)
        records.append(f"{role.name} {role.service} {role.executable} {broker_sha} {broker_cd} {role.worker} {worker_sha} {worker_cd}")
    (PAYLOAD / "manifest.txt").write_text("\n".join(records) + "\n", encoding="ascii")
    verify_staged()


def verify_sources(payload):
    lock = payload / "source.sha256"
    regular(lock)
    if lock.stat().st_size > 64 * 1024 or (payload / "source").is_symlink():
        raise ValueError("Invalid worker broker source notice boundary")
    names = set()
    for line in lock.read_text(encoding="ascii").splitlines():
        fields = line.split("  ")
        if (len(fields) != 2 or re.fullmatch(r"[0-9a-f]{64}", fields[0]) is None
                or not fields[1].startswith("source/") or ".." in Path(fields[1]).parts
                or fields[1] in names or digest(payload / fields[1]) != fields[0]):
            raise ValueError("Invalid or modified worker broker source notice")
        names.add(fields[1])
    expected = {"source/LICENSE", "source/README.txt"}
    for path in NATIVE.rglob("*"):
        if path.is_file():
            name = "source/" + path.relative_to(NATIVE).as_posix()
            expected.add(name)
            if digest(payload / name) != digest(path):
                raise ValueError("Bundled broker source differs from current reviewed build input")
    if names != expected or digest(payload / "source/LICENSE") != digest(ROOT / "LICENSE"):
        raise ValueError("Missing or extra worker broker source/license notice")
    tree=list((payload / "source").rglob("*"))
    if any(path.is_symlink() for path in tree) or {
            path.relative_to(payload).as_posix() for path in tree if path.is_file()} != expected:
        raise ValueError("Unexpected or indirect worker broker source notice file")


def verify_services(payload, services):
    records = parse((payload / "manifest.txt").read_text(encoding="ascii"))
    verify_sources(payload)
    for role, fields in records:
        service = services / f"{role.service}.xpc"
        expected_files={"Contents/Info.plist", f"Contents/MacOS/{role.executable}",
                        f"Contents/MacOS/{role.worker}", "Contents/_CodeSignature/CodeResources"}
        if service.is_symlink() or any(path.is_symlink() for path in service.rglob("*")):
            raise ValueError("Indirect worker service bundle")
        actual_files={path.relative_to(service).as_posix() for path in service.rglob("*") if path.is_file()}
        if actual_files != expected_files:
            raise ValueError("Worker service must contain only fixed metadata, code and signature")
        regular(service / "Contents/Info.plist")
        if plistlib.loads((service / "Contents/Info.plist").read_bytes()) != info(role):
            raise ValueError("Worker service metadata/policy differs")
        subprocess.run(["/usr/bin/codesign", "--verify", "--strict", str(service)], check=True, capture_output=True)
        for name, sha, cd in [(role.executable, fields[3], fields[4]), (role.worker, fields[6], fields[7])]:
            path = service / "Contents/MacOS" / name
            regular(path)
            if identity(path) != (sha, cd):
                raise ValueError("Worker broker/helper identity differs from loaded-host pins")
            assert_executable(path)
        entitlements = subprocess.run(["/usr/bin/codesign", "-d", "--entitlements", ":-", str(service)], check=True, capture_output=True).stdout
        if entitlements.strip() and plistlib.loads(entitlements):
            raise ValueError("Worker broker must have no inherited App Sandbox, network or container entitlement")
    return records


def verify_staged():
    return verify_services(PAYLOAD, PAYLOAD / "services")


def prepare_bundle(application):
    payload = application / "Contents/Resources/worker-brokers"
    source = payload / "services"
    verify_services(payload, source)
    target = application / "Contents/XPCServices"
    target.mkdir(exist_ok=True)
    for role in ROLES:
        destination = target / f"{role.service}.xpc"
        if destination.exists() or destination.is_symlink():
            raise ValueError("Refusing to replace an existing staged worker service")
        shutil.move(str(source / destination.name), destination)
    source.rmdir()


def verify_bundle(application, unsigned=False):
    payload = application / "Contents/Resources/worker-brokers"
    services=payload / "services" if unsigned else application / "Contents/XPCServices"
    records = verify_services(payload, services)
    manifest = (payload / "manifest.txt").read_text(encoding="ascii")
    if embedded_manifest(application / "Contents/MacOS/zeta-desktop", b"__worker_brokers") != manifest.splitlines():
        raise ValueError("Bundled worker brokers differ from the running-host image manifest")
    return records


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--identity", default="-")
    parser.add_argument("--verify-staged", action="store_true")
    parser.add_argument("--prepare-bundle", type=Path)
    parser.add_argument("--verify-bundle", type=Path)
    parser.add_argument("--verify-unsigned-bundle", type=Path)
    arguments = parser.parse_args()
    if arguments.verify_staged:
        verify_staged()
    elif arguments.prepare_bundle:
        prepare_bundle(arguments.prepare_bundle)
    elif arguments.verify_bundle:
        verify_bundle(arguments.verify_bundle)
    elif arguments.verify_unsigned_bundle:
        verify_bundle(arguments.verify_unsigned_bundle, unsigned=True)
    else:
        stage(arguments.identity)


if __name__ == "__main__":
    main()
