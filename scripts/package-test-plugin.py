#!/usr/bin/env python3
"""Create a local-only lifecycle package from an explicitly built test plugin."""

import ctypes
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import sys
import tarfile


class ByteSlice(ctypes.Structure):
    _fields_ = [("ptr", ctypes.c_void_p), ("length", ctypes.c_uint64)]


class Descriptor(ctypes.Structure):
    _fields_ = [
        ("abi_version", ctypes.c_uint32),
        ("struct_size", ctypes.c_uint32),
        ("manifest", ByteSlice),
        ("dispatch", ctypes.c_void_p),
        ("free_buffer", ctypes.c_void_p),
    ]


def digest(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def main():
    if len(sys.argv) not in (4, 5):
        raise ValueError("usage: package-test-plugin.py LIBRARY PLUGIN_SOURCE OUTPUT [FIXTURE_ID]")
    os.umask(0o077)
    library_path, source, output = (Path(value).resolve() for value in sys.argv[1:4])
    fixture_id = sys.argv[4] if len(sys.argv) == 5 else "codex"
    if fixture_id not in ("codex", "example-response-adapter"):
        raise ValueError("unsupported trusted fixture")
    # This is an explicitly built trusted fixture, never an uploaded/discovered package.
    library = ctypes.CDLL(str(library_path))
    entry = library.ai_gateway_connector_entry_v1
    entry.restype = ctypes.POINTER(Descriptor)
    pointer = entry()
    if not pointer:
        raise ValueError("missing native descriptor")
    descriptor = pointer.contents
    if descriptor.abi_version != 1 or descriptor.struct_size != ctypes.sizeof(Descriptor):
        raise ValueError("unsupported fixture ABI")
    if not descriptor.manifest.ptr or not 0 < descriptor.manifest.length <= 65536:
        raise ValueError("invalid fixture manifest")
    manifest = json.loads(ctypes.string_at(descriptor.manifest.ptr, descriptor.manifest.length))
    if manifest["id"] != fixture_id:
        raise ValueError("unexpected trusted fixture identity")
    architecture = {"x86_64": "x86_64", "aarch64": "aarch64"}.get(platform.machine())
    if not architecture or platform.system() != "Linux":
        raise ValueError("unsupported fixture platform")
    sha256 = digest(library_path)
    directory = output / "plugin-directory" / "artifacts" / fixture_id / sha256
    directory.mkdir(parents=True, exist_ok=True, mode=0o700)
    if not (directory / "SHA256SUMS").exists():
        shutil.copyfile(library_path, directory / library_path.name)
        shutil.copyfile(source / "LICENSE", directory / "LICENSE")
        (directory / "LICENSES" / "fixture").mkdir(parents=True, mode=0o700)
        shutil.copyfile(source / "LICENSE", directory / "LICENSES" / "fixture" / "LICENSE")
        (directory / "THIRD_PARTY_NOTICES.md").write_text(
            "Synthetic local test package, not a redistributable release.\n"
            "Use the plugin repository release tooling for complete third-party notices.\n"
        )
        (directory / "manifest.json").write_text(json.dumps(manifest, sort_keys=True))
        (directory / "build-info.json").write_text(json.dumps({
            "schema_version": 1,
            "connector_abi": 1,
            "connector_version": manifest["version"],
            "target": f"{architecture}-unknown-linux-gnu",
            "library": library_path.name,
            "library_sha256": sha256,
        }, sort_keys=True))
        files = sorted(path for path in directory.rglob("*") if path.is_file())
        (directory / "SHA256SUMS").write_text("".join(
            f"{digest(path)}  {path.relative_to(directory).as_posix()}\n" for path in files
        ))
        for path in directory.rglob("*"):
            if path.is_file():
                path.chmod(0o444)
    archive = output / f"{fixture_id}-test.tar.gz"
    with tarfile.open(archive, "w:gz") as destination:
        destination.add(directory, arcname=f"{fixture_id}-test")


if __name__ == "__main__":
    main()
