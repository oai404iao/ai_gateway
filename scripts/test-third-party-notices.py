#!/usr/bin/env python3
"""Check project-owned workspace license text inclusion without Cargo or npm."""

import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch


SPEC = importlib.util.spec_from_file_location(
    "notices", Path(__file__).with_name("generate-third-party-notices.py")
)
NOTICES = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(NOTICES)


class WorkspaceLicenseTests(unittest.TestCase):
    def test_sdk_inherits_full_project_license_not_manifest_fallback(self):
        root = NOTICES.ROOT
        metadata = {
            "workspace_members": ["gateway", "sdk"],
            "packages": [
                {"id": "gateway", "name": "ai-gateway", "version": "0.1.0",
                 "manifest_path": str(root / "Cargo.toml"), "license": "AGPL-3.0-only"},
                {"id": "sdk", "name": "ai-gateway-connector-sdk", "version": "0.1.0",
                 "manifest_path": str(root / "crates/connector-sdk/Cargo.toml"),
                 "license": "AGPL-3.0-only"},
            ],
            "resolve": {"nodes": [
                {"id": "gateway", "deps": [{"pkg": "sdk", "dep_kinds": [{"kind": None}]}]},
                {"id": "sdk", "deps": []},
            ]},
        }
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary)
            with patch.object(NOTICES, "run", return_value=json.dumps(metadata)), \
                    patch.object(NOTICES, "OUTPUT", output, create=True):
                packages = NOTICES.cargo_packages()
            self.assertEqual(len(packages), 1)
            self.assertEqual(len(packages[0][4]), 1)
            material = output / packages[0][4][0]
            self.assertEqual(material.name, "LICENSE")
            self.assertEqual(material.read_bytes(), (root / "LICENSE").read_bytes())

    def test_third_party_missing_license_keeps_existing_manifest_fallback(self):
        with tempfile.TemporaryDirectory() as temporary:
            source = Path(temporary) / "source"
            source.mkdir()
            (source / "Cargo.toml").write_text('[package]\nname = "third-party"\n')
            output = Path(temporary) / "output"
            materials = NOTICES.copy_materials(output, "cargo", "third-party", "1", source)
            self.assertEqual(len(materials), 1)
            self.assertEqual(Path(materials[0]).name, "UPSTREAM_Cargo.toml")


if __name__ == "__main__":
    unittest.main()
