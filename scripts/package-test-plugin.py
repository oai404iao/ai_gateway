#!/usr/bin/env python3
"""Create a local-only lifecycle package from an explicitly built test plugin."""

import argparse
import ctypes
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
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
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("library", type=Path)
    parser.add_argument("source", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("fixture_id", nargs="?", default="codex",
                        choices=("codex", "example-response-adapter", "example-usage-parser"))
    parser.add_argument("--pin", type=Path)
    args = parser.parse_args()
    os.umask(0o077)
    library_path, source, output = (value.resolve() for value in (args.library, args.source, args.output))
    fixture_id = args.fixture_id
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
        partial = list(directory.rglob("*"))
        if directory.is_symlink() or any(path.is_symlink() for path in partial):
            raise ValueError("unexpected symlink in partial fixture package")
        # Restored caches can retain readonly files from an interrupted package.
        for path in partial:
            if path.is_file():
                path.chmod(0o600)
        shutil.copyfile(library_path, directory / library_path.name)
        shutil.copyfile(source / "LICENSE", directory / "LICENSE")
        (directory / "LICENSES" / "fixture").mkdir(parents=True, exist_ok=True, mode=0o700)
        shutil.copyfile(source / "LICENSE", directory / "LICENSES" / "fixture" / "LICENSE")
        (directory / "THIRD_PARTY_NOTICES.md").write_text(
            "Synthetic local test package, not a redistributable release.\n"
            "Use the plugin repository release tooling for complete third-party notices.\n"
        )
        (directory / "manifest.json").write_text(json.dumps(manifest, sort_keys=True))
        (directory / "build-info.json").write_text(json.dumps({
            "schema_version": 1,
            "connector_abi": descriptor.abi_version,
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
    if args.pin:
        pin = json.loads(args.pin.read_text())
        packaged = json.loads((directory / "manifest.json").read_text())
        info = json.loads((directory / "build-info.json").read_text())
        if packaged != manifest or info["connector_abi"] != descriptor.abi_version:
            raise ValueError("packaged metadata does not match native descriptor")
        actual = {"plugin_id": packaged["id"], "plugin_version": packaged["version"],
                  "abi_version": info["connector_abi"], "protocol_version": packaged["protocol_version"]}
        if actual != {key: pin[key] for key in actual}:
            raise ValueError(f"packaged metadata {actual} does not match {args.pin}")
    archive = output / f"{fixture_id}-test.tar.gz"
    with tarfile.open(archive, "w:gz") as destination:
        destination.add(directory, arcname=f"{fixture_id}-test")


if __name__ == "__main__":
    main()
