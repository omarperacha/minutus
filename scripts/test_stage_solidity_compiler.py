"""Synthetic bundle tests never create a compiler/signing authority."""
import hashlib
import importlib.util
from pathlib import Path
import shutil
import struct
import tempfile
import unittest
from unittest.mock import patch

import stage_solidity_compiler as compiler


def helper_image(commands=None, cpu=0x0100000C, kind=2, subtype=0):
    if commands is None:
        commands = [struct.pack("<IIIIII", 0x32, 24, 1, 13 << 16 | 3 << 8, 0, 0),
                    dependency("/usr/lib/libSystem.B.dylib")]
    data = b"".join(commands)
    return struct.pack("<IiiIIIII", 0xFEEDFACF, cpu, subtype, kind, len(commands), len(data), 0, 0) + data


def dependency(name, command=0xC):
    value = name.encode("ascii") + b"\0"
    size = (24 + len(value) + 7) // 8 * 8
    return struct.pack("<IIIIII", command, size, 24, 0, 0, 0) + value + b"\0" * (size - 24 - len(value))


def host_image(records):
    text = ("\n".join(records) + "\n").encode("ascii")
    segment = struct.pack("<II16sQQQQiiII", 0x19, 152, b"__TEXT", 0, 0, 0, 0, 7, 5, 1, 0)
    section = struct.pack("<16s16sQQIIIIIIII", b"__solc_worker", b"__TEXT", 0, len(text), 184, 0, 0, 0, 0, 0, 0, 0)
    header = struct.pack("<IiiIIIII", 0xFEEDFACF, 0x0100000C, 0, 2, 1, 152, 0, 0)
    return header + segment + section + text


class CompilerBundleTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.app = self.root / "App.app"
        self.macos = self.app / "Contents/MacOS"
        self.macos.mkdir(parents=True)
        shutil.copytree(compiler.ROOT / "src-tauri/third-party-notices",
                        self.app / "Contents/Resources/third-party-notices")
        self.helper = self.macos / compiler.NAME
        self.helper.write_bytes(helper_image())
        self.digest = hashlib.sha256(self.helper.read_bytes()).hexdigest()
        self.records = ["minutus-solidity-compiler-v1",
                        f"{compiler.NAME} {self.digest} {'b' * 40}"]
        self.host = self.macos / "zeta-desktop"
        self.host.write_bytes(host_image(self.records))

    @staticmethod
    def synthetic_identity(path):
        return hashlib.sha256(path.read_bytes()).hexdigest(), "b" * 40

    def verify(self):
        with patch.object(compiler, "identity", side_effect=self.synthetic_identity):
            compiler.verify_bundle(self.app)

    def test_complete_bundle_uses_actual_embedded_section_and_real_notice_tree(self):
        self.verify()

    def test_replaced_missing_and_symlinked_helpers_reject(self):
        self.helper.write_bytes(b"replacement")
        with self.assertRaises(ValueError):
            self.verify()
        self.helper.unlink()
        with self.assertRaises(OSError):
            self.verify()
        replacement = self.macos / "elsewhere"
        replacement.write_bytes(helper_image())
        self.helper.symlink_to(replacement)
        with self.assertRaises((ValueError, OSError)):
            self.verify()

    def test_wrong_code_directory_hash_rejects_even_when_file_digest_matches(self):
        with patch.object(compiler, "identity", return_value=(self.digest, "c" * 40)):
            with self.assertRaises(ValueError):
                compiler.verify_bundle(self.app)

    def test_source_side_manifest_never_replaces_missing_or_malformed_host_pins(self):
        mutable = self.root / "src-tauri/solidity-compiler"
        mutable.mkdir(parents=True)
        (mutable / "manifest.txt").write_text("\n".join(self.records) + "\n")
        for records in [[], ["wrong", self.records[1]], self.records + ["extra"],
                        [self.records[0], "wrong " + "a" * 64 + " " + "b" * 40],
                        [self.records[0], compiler.NAME],
                        [self.records[0], f"{compiler.NAME} {'a' * 64} {'b' * 40}"]]:
            with self.subTest(records=records):
                self.host.write_bytes(host_image(records))
                with self.assertRaises(ValueError):
                    self.verify()

    def test_missing_tampered_or_indirect_compiler_resources_reject(self):
        resources = self.app / "Contents/Resources/third-party-notices/solidity-compiler"
        for notice in sorted(resources.iterdir()):
            if not notice.is_file():
                continue
            original = notice.read_bytes()
            with self.subTest(notice=notice.name):
                notice.unlink()
                with self.assertRaises((ValueError, OSError)):
                    self.verify()
                notice.write_bytes(original + b"tampered")
                with self.assertRaises(ValueError):
                    self.verify()
                notice.unlink()
                external = self.root / (notice.name + ".external")
                external.write_bytes(original)
                notice.symlink_to(external)
                with self.assertRaises(ValueError):
                    self.verify()
                notice.unlink()
                notice.write_bytes(original)
        self.verify()


class CompilerStageTests(unittest.TestCase):
    def test_dynamic_signing_identity_and_missing_notices_fail_before_build(self):
        with patch.object(compiler.subprocess, "run") as run:
            with self.assertRaises(ValueError):
                compiler.stage("caller-selected certificate name")
            run.assert_not_called()
            with patch.object(compiler, "verify_notice_sources", side_effect=ValueError("missing notices")):
                with self.assertRaisesRegex(ValueError, "missing notices"):
                    compiler.stage()
            run.assert_not_called()

    def test_stage_rejects_indirect_destination_before_build(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            binaries = root / "src-tauri/binaries"
            binaries.mkdir(parents=True)
            (binaries / f"{compiler.NAME}-{compiler.TARGET}").symlink_to(root / "elsewhere")
            with patch.object(compiler, "ROOT", root), patch.object(compiler.subprocess, "run") as run, patch.object(compiler, "verify_executable"):
                with self.assertRaisesRegex(ValueError, "indirect"):
                    compiler.stage()
                run.assert_not_called()

    def test_manifest_is_written_only_after_successful_build_and_identity(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "src-tauri").mkdir()
            manifest = root / "src-tauri/solidity-compiler/manifest.txt"
            with patch.object(compiler, "ROOT", root), patch.object(compiler.subprocess, "run") as run, patch.object(compiler, "verify_executable") as verify:
                run.side_effect = RuntimeError("build failed")
                with self.assertRaises(RuntimeError):
                    compiler.stage()
                self.assertFalse(manifest.exists())
                run.side_effect = None
                with patch.object(compiler, "identity", side_effect=ValueError("invalid signature")):
                    with self.assertRaises(ValueError):
                        compiler.stage()
                self.assertFalse(manifest.exists())
                with patch.object(compiler, "identity", return_value=("a" * 64, "b" * 40)):
                    compiler.stage()
                self.assertEqual(manifest.read_text(),
                                 f"minutus-solidity-compiler-v1\n{compiler.NAME} {'a' * 64} {'b' * 40}\n")
                self.assertFalse(manifest.with_name("manifest.txt.incomplete").exists())
                self.assertEqual(verify.call_count, 2)


class CompilerExecutableTests(unittest.TestCase):
    def check(self, data, rejected=False):
        with tempfile.TemporaryDirectory() as directory:
            helper = Path(directory) / "helper"
            helper.write_bytes(data)
            if rejected:
                with self.assertRaises((ValueError, OSError)):
                    compiler.verify_executable(helper)
            else:
                compiler.verify_executable(helper)

    def test_thin_arm64_macos_13_3_system_dependencies_pass(self):
        self.check(helper_image())

    def test_wrong_architecture_file_kind_and_arm64e_reject(self):
        for image in [helper_image(cpu=0x01000007), helper_image(kind=6), helper_image(subtype=2), b"\xca\xfe\xba\xbe" + bytes(64)]:
            self.check(image, rejected=True)

    def test_missing_duplicate_wrong_platform_and_newer_deployment_reject(self):
        base = [struct.pack("<IIIIII", 0x32, 24, 1, 13 << 16 | 3 << 8, 0, 0), dependency("/usr/lib/libSystem.B.dylib")]
        for commands in [[base[1]], [base[0], base[0], base[1]],
                         [struct.pack("<IIIIII", 0x32, 24, 2, 13 << 16 | 3 << 8, 0, 0), base[1]],
                         [struct.pack("<IIIIII", 0x32, 24, 1, 26 << 16, 0, 0), base[1]],
                         [struct.pack("<IIIIII", 0x32, 24, 1, 13 << 16 | 3 << 8, 0, 1), base[1]]]:
            self.check(helper_image(commands), rejected=True)

    def test_all_dylib_variants_reject_non_system_and_traversal_paths(self):
        version = struct.pack("<IIIIII", 0x32, 24, 1, 13 << 16 | 3 << 8, 0, 0)
        for command in [0xC, 0x80000018, 0x8000001F, 0x20, 0x80000023]:
            for path in ["@rpath/libevil.dylib", "/tmp/evil.dylib", "/usr/lib/../../tmp/evil.dylib", "/System/LibraryEvil/lib.dylib"]:
                self.check(helper_image([version, dependency(path, command)]), rejected=True)

    def test_truncated_malformed_unterminated_and_excessive_load_commands_reject(self):
        for image in [helper_image()[:-1], helper_image([struct.pack("<II", 0x32, 7)]),
                      helper_image([struct.pack("<IIIIII", 0xC, 24, 99, 0, 0, 0)]),
                      helper_image([struct.pack("<IIIIII", 0xC, 32, 24, 0, 0, 0) + b"nonul!!!"]),
                      struct.pack("<IiiIIIII", 0xFEEDFACF, 0x0100000C, 0, 2, 4097, 0, 0, 0),
                      struct.pack("<IiiIIIII", 0xFEEDFACF, 0x0100000C, 0, 2, 1, 4 * 1024 * 1024 + 1, 0, 0)]:
            self.check(image, rejected=True)

    def test_old_platform_commands_and_loader_overrides_reject(self):
        version = struct.pack("<IIIIII", 0x32, 24, 1, 13 << 16 | 3 << 8, 0, 0)
        native = dependency("/usr/lib/libSystem.B.dylib")
        for command in [0x24, 0x25, 0x2F, 0x30, 0x27, 0xD]:
            self.check(helper_image([version, native, struct.pack("<II", command, 8)]), rejected=True)
        for command in [0xE, 0x8000001C]:
            name = b"/tmp/evil\0"
            size = (12 + len(name) + 7) // 8 * 8
            load = struct.pack("<III", command, size, 12) + name + bytes(size - 12 - len(name))
            self.check(helper_image([version, native, load]), rejected=True)

    def test_executable_file_bound_and_symlink_reject(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "helper"
            path.write_bytes(helper_image())
            link = Path(directory) / "indirect"
            link.symlink_to(path)
            with self.assertRaises(OSError):
                compiler.verify_executable(link)
            with path.open("r+b") as stream:
                stream.truncate(16 * 1024 * 1024 + 1)
            with self.assertRaises(ValueError):
                compiler.verify_executable(path)

    def test_build_script_pins_minimum_target_and_reviewed_entitlements(self):
        path = compiler.ROOT / "crates/solidity-compilation/native/build.py"
        spec = importlib.util.spec_from_file_location("compiler_native_build", path)
        build = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(build)
        with tempfile.TemporaryDirectory() as directory, patch.object(build.subprocess, "run") as run:
            build.build(Path(directory) / "helper", "a" * 40)
            compile_command = run.call_args_list[0].args[0]
            self.assertEqual(compile_command[compile_command.index("-target") + 1], "arm64-apple-macosx13.3")
            signing = run.call_args_list[1].args[0]
            self.assertEqual(signing[signing.index("--entitlements") + 1], str(path.with_name("SolidityCompiler.entitlements.plist")))
            self.assertIn("runtime", signing)
            self.assertIn("--digest-algorithm=sha256", signing)


if __name__ == "__main__":
    unittest.main()
