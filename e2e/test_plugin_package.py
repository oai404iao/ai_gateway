"""Offline recovery and reuse checks for explicitly trusted fixture packages."""

import ctypes
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import stat
import sys
import tarfile
import tempfile
import unittest
from unittest.mock import Mock, patch

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("package_test_plugin", ROOT / "scripts/package-test-plugin.py")
PACKAGE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(PACKAGE)


class PluginPackageTests(unittest.TestCase):
    def invoke(self, library, source, output, fixture_id):
        manifest = {"id": fixture_id, "version": "0.2.0", "protocol_version": 3,
                    "operations": ["responses"], "commands": ["attempt.describe/v1"]}
        payload = json.dumps(manifest).encode()
        buffer = ctypes.create_string_buffer(payload)
        descriptor = PACKAGE.Descriptor(
            1, ctypes.sizeof(PACKAGE.Descriptor),
            PACKAGE.ByteSlice(ctypes.addressof(buffer), len(payload)), 1, 1,
        )
        native = Mock()
        native.ai_gateway_connector_entry_v1.return_value = ctypes.pointer(descriptor)
        args = ["package-test-plugin.py", str(library), str(source), str(output)]
        if fixture_id != "codex":
            args.append(fixture_id)
        previous = os.umask(0o077)
        try:
            with patch.object(sys, "argv", args), patch.object(PACKAGE.ctypes, "CDLL", return_value=native), \
                    patch.object(PACKAGE.platform, "machine", return_value="x86_64"), \
                    patch.object(PACKAGE.platform, "system", return_value="Linux"):
                PACKAGE.main()
        finally:
            os.umask(previous)
        return manifest

    def assert_package(self, directory, archive, library, license_file, manifest):
        expected_build = {
            "schema_version": 1, "connector_abi": 1, "connector_version": manifest["version"],
            "target": "x86_64-unknown-linux-gnu", "library": library.name,
            "library_sha256": hashlib.sha256(library.read_bytes()).hexdigest(),
        }
        self.assertEqual(json.loads((directory / "manifest.json").read_text()), manifest)
        self.assertEqual(json.loads((directory / "build-info.json").read_text()), expected_build)
        self.assertEqual((directory / library.name).read_bytes(), library.read_bytes())
        self.assertEqual((directory / "LICENSE").read_bytes(), license_file.read_bytes())
        self.assertEqual((directory / "LICENSES/fixture/LICENSE").read_bytes(), license_file.read_bytes())
        expected_files = {
            library.name, "LICENSE", "LICENSES/fixture/LICENSE", "THIRD_PARTY_NOTICES.md",
            "manifest.json", "build-info.json",
        }
        checksums = {}
        for line in (directory / "SHA256SUMS").read_text().splitlines():
            digest, name = line.split("  ", 1)
            checksums[name] = digest
            self.assertEqual(digest, hashlib.sha256((directory / name).read_bytes()).hexdigest())
        self.assertEqual(set(checksums), expected_files)
        for path in directory.rglob("*"):
            if path.is_file():
                self.assertEqual(stat.S_IMODE(path.stat().st_mode), 0o444)
        with tarfile.open(archive) as package:
            members = {member.name.split("/", 1)[1]: member for member in package.getmembers()
                       if member.isfile()}
            self.assertEqual(set(members), expected_files | {"SHA256SUMS"})
            for name, member in members.items():
                self.assertEqual(member.mode, 0o444)
                payload = package.extractfile(member).read()
                self.assertEqual(payload, (directory / name).read_bytes())
                if name != "SHA256SUMS":
                    self.assertEqual(hashlib.sha256(payload).hexdigest(), checksums[name])

    def test_partial_readonly_cache_recovers_and_complete_package_reuses_without_rewriting(self):
        for fixture_id in ("codex", "example-response-adapter", "example-usage-parser"):
            with self.subTest(fixture_id=fixture_id), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                library = root / "fixture.so"
                library.write_bytes(b"trusted local fixture")
                source = root / "source"
                source.mkdir()
                license_file = source / "LICENSE"
                license_file.write_text("Synthetic fixture license\n")
                output = root / "output"
                digest = hashlib.sha256(library.read_bytes()).hexdigest()
                directory = output / "plugin-directory/artifacts" / fixture_id / digest
                (directory / "LICENSES/fixture").mkdir(parents=True)
                for name in (library.name, "LICENSE", "LICENSES/fixture/LICENSE",
                             "manifest.json", "build-info.json", "THIRD_PARTY_NOTICES.md"):
                    path = directory / name
                    path.write_bytes(b"interrupted readonly package")
                    path.chmod(0o444)
                self.assertFalse((directory / "SHA256SUMS").exists())
                manifest = self.invoke(library, source, output, fixture_id)
                archive = output / f"{fixture_id}-test.tar.gz"
                self.assert_package(directory, archive, library, license_file, manifest)
                before = {str(path.relative_to(directory)): (path.read_bytes(), path.stat().st_mtime_ns)
                          for path in directory.rglob("*") if path.is_file()}
                with patch.object(PACKAGE.shutil, "copyfile", side_effect=AssertionError("complete package rewritten")):
                    self.invoke(library, source, output, fixture_id)
                after = {str(path.relative_to(directory)): (path.read_bytes(), path.stat().st_mtime_ns)
                         for path in directory.rglob("*") if path.is_file()}
                self.assertEqual(before, after)
                self.assert_package(directory, archive, library, license_file, manifest)

    def test_partial_cache_symlink_is_rejected_without_touching_external_file(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            library = root / "fixture.so"
            library.write_bytes(b"trusted local fixture")
            source = root / "source"
            source.mkdir()
            (source / "LICENSE").write_text("Synthetic fixture license\n")
            output = root / "output"
            digest = hashlib.sha256(library.read_bytes()).hexdigest()
            directory = output / "plugin-directory/artifacts/codex" / digest
            directory.mkdir(parents=True)
            external = root / "external"
            external.write_bytes(b"must remain unchanged")
            external.chmod(0o444)
            (directory / "LICENSE").symlink_to(external)
            with self.assertRaisesRegex(ValueError, "symlink"):
                self.invoke(library, source, output, "codex")
            self.assertEqual(external.read_bytes(), b"must remain unchanged")
            self.assertEqual(stat.S_IMODE(external.stat().st_mode), 0o444)
            self.assertFalse((directory / "SHA256SUMS").exists())


if __name__ == "__main__":
    unittest.main()
