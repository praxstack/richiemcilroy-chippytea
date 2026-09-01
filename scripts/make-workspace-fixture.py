#!/usr/bin/env python3
"""Create a disposable npm, Bun or pnpm workspace for shared-lock parsing."""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import sys
import time


HELPER_PATH = Path(__file__).with_name("make-fixture.py")
SPEC = importlib.util.spec_from_file_location("chippytea_fixture", HELPER_PATH)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError("Cannot load the adjacent fixture helpers")
fixture = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(fixture)

PAYLOAD_BYTES = 100 * 1024**2
CHUNK_BYTES = 1024**2
WORKSPACE_NAME = "chippytea-shared-lock-fixture"


def json_bytes(value: object) -> bytes:
    return json.dumps(value, separators=(",", ":")).encode()


def member_name(number: int) -> str:
    return f"chippytea-member-{number:04d}"


def shared_lock(members: int, size: int) -> tuple[bytes, int]:
    """Fill most of the requested size with package records, then JSON whitespace."""
    parts = [
        b'{"name":' + json_bytes(WORKSPACE_NAME)
        + b',"version":"1.0.0","lockfileVersion":3,"requires":true,"packages":{',
        b'"":' + json_bytes({
            "name": WORKSPACE_NAME, "version": "1.0.0", "workspaces": ["packages/*"],
        }),
    ]
    for number in range(members):
        relative = f"packages/member-{number:04d}"
        parts.append(b"," + json_bytes(relative) + b":" + json_bytes({
            "name": member_name(number), "version": "1.0.0",
        }))
        parts.append(b"," + json_bytes(f"node_modules/{member_name(number)}") + b":" + json_bytes({
            "resolved": relative, "link": True,
        }))
    ending = b"}}\n"
    used = sum(map(len, parts)) + len(ending)
    if used > size:
        raise ValueError("The requested lock size cannot contain the workspace member table")
    dependencies = 0
    while True:
        name = f"synthetic-dependency-{dependencies:06d}"
        entry = b"," + json_bytes(f"node_modules/{name}") + b":" + json_bytes({
            "version": "1.0.0",
            "resolved": f"https://example.invalid/chippytea/{name}-1.0.0.tgz",
            "dev": True,
        })
        if used + len(entry) > size:
            break
        parts.append(entry)
        used += len(entry)
        dependencies += 1
    parts.extend((ending, b" " * (size - used)))
    return b"".join(parts), dependencies


def shared_bun_lock(members: int, size: int) -> tuple[bytes, int]:
    """Create a synthetic Bun text lock with exact member names and byte size."""
    workspaces = {"": {"name": WORKSPACE_NAME}}
    workspaces.update({
        f"packages/member-{number:04d}": {"name": member_name(number)}
        for number in range(members)
    })
    parts = [
        b'{"lockfileVersion":1,"workspaces":' + json_bytes(workspaces)
        + b',"packages":{',
    ]
    ending = b"}}\n"
    used = sum(map(len, parts)) + len(ending)
    if used > size:
        raise ValueError("The requested lock size cannot contain the workspace member table")
    dependencies = 0
    while True:
        name = f"synthetic-dependency-{dependencies:06d}"
        entry = (b"," if dependencies else b"") + json_bytes(name) + b":" + json_bytes([
            f"{name}@1.0.0",
            f"https://example.invalid/chippytea/{name}-1.0.0.tgz",
            {},
            "sha512-synthetic-fixture-not-for-installation",
        ])
        if used + len(entry) > size:
            break
        parts.append(entry)
        used += len(entry)
        dependencies += 1
    parts.extend((ending, b" " * (size - used)))
    return b"".join(parts), dependencies


def shared_pnpm_lock(members: int, size: int) -> tuple[bytes, int]:
    """Create synthetic pnpm importer/package records with an exact byte size."""
    parts = [b"lockfileVersion: '9.0'\n\nimporters:\n  .: {}\n"]
    parts.extend(
        f"  packages/member-{number:04d}: {{}}\n".encode()
        for number in range(members)
    )
    parts.append(b"\npackages:\n")
    ending = b"\nsnapshots: {}\n"
    used = sum(map(len, parts)) + len(ending)
    if used > size:
        raise ValueError("The requested lock size cannot contain the workspace member table")
    dependencies = 0
    while True:
        entry = (
            f"  synthetic-dependency-{dependencies:06d}@1.0.0:\n"
            "    resolution: {integrity: sha512-synthetic-fixture-not-for-installation}\n"
        ).encode()
        if used + len(entry) > size:
            break
        parts.append(entry)
        used += len(entry)
        dependencies += 1
    parts.extend((ending, b" " * (size - used)))
    return b"".join(parts), dependencies


def main(argv: list[str] | None = None) -> int:
    arguments = argparse.ArgumentParser(description=__doc__)
    arguments.add_argument("path", type=Path, help="A NEW directory for this fixture")
    arguments.add_argument("--members", type=int, default=128, help="Workspace members, 2..512 (default: 128)")
    arguments.add_argument("--lock-kib", type=int, default=1024, help="Exact shared lock size, 256..4096 KiB (default: 1024)")
    arguments.add_argument("--lock-format", choices=("npm", "bun", "pnpm"), default="npm", help="Shared lock format (default: npm)")
    arguments.add_argument("--age-days", type=int, default=8, help="Age all fixture modification times, 8..3650 days (default: 8)")
    args = arguments.parse_args(argv)
    if not 2 <= args.members <= 512:
        arguments.error("--members must be between 2 and 512")
    if not 256 <= args.lock_kib <= 4096:
        arguments.error("--lock-kib must be between 256 and 4096")
    if not 8 <= args.age_days <= 3650:
        arguments.error("--age-days must be between 8 and 3650")

    requested = args.path.expanduser().absolute()
    if os.path.lexists(requested):
        arguments.error(f"Refusing an existing path: {requested}")
    lock_builder, lock_name = {
        "npm": (shared_lock, "package-lock.json"),
        "bun": (shared_bun_lock, "bun.lock"),
        "pnpm": (shared_pnpm_lock, "pnpm-lock.yaml"),
    }[args.lock_format]
    lock, dependencies = lock_builder(args.members, args.lock_kib * 1024)
    workspace_config = b"packages:\n  - 'packages/*'\n" if args.lock_format == "pnpm" else None
    ancestor = requested.parent
    while not ancestor.exists():
        ancestor = ancestor.parent
    # Include directory and file metadata headroom beyond the actual payload.
    directories = 2 + 2 * args.members  # baseline, packages, members, artifacts.
    files = args.members + 3 + int(workspace_config is not None)
    projected = PAYLOAD_BYTES + len(lock) + directories * 18_432 + files * 8192
    fixture.require_space(ancestor, projected)
    requested.parent.mkdir(parents=True, exist_ok=True)
    root = requested.parent.resolve() / requested.name
    root.mkdir(mode=0o700, exist_ok=False)
    marker_path = root / fixture.MARKER
    marker = {
        "magic": fixture.MAGIC,
        "schema_version": 1,
        "status": "creating",
        "created_at": fixture.utc_now(),
        "generator_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
        "fixture_helper_sha256": hashlib.sha256(HELPER_PATH.read_bytes()).hexdigest(),
        "baseline_relative_path": "baseline",
        "minimum_free_bytes": fixture.MIN_FREE_BYTES,
        "fixture_kind": f"{args.lock_format}-shared-lock-workspace",
        "workspace_members": args.members,
        "lock_format": args.lock_format,
        "shared_lock_filename": lock_name,
        "shared_lock_bytes": len(lock),
        "shared_lock_sha256": hashlib.sha256(lock).hexdigest(),
        "synthetic_dependency_entries": dependencies,
        "payload_files": 1,
        "bytes_per_payload_file": PAYLOAD_BYTES,
        "empty_artifacts": args.members - 1,
        "expected_eligible_relative_paths": ["packages/member-0000/node_modules"],
        "synthetic_modification_age_days": args.age_days,
        "cache_state": "Unknown; creation itself populates filesystem caches.",
    }
    fixture.write_new(marker_path, json_bytes(marker) + b"\n")
    started = time.monotonic()
    members_created = payload_written = logical_bytes = 0
    try:
        baseline = root / "baseline"
        baseline.mkdir()
        manifest = json_bytes({
            "name": WORKSPACE_NAME, "version": "1.0.0", "private": True,
            "workspaces": ["packages/*"],
        }) + b"\n"
        fixture.write_new(baseline / "package.json", manifest)
        fixture.write_new(baseline / lock_name, lock)
        logical_bytes += len(manifest) + len(lock)
        if workspace_config is not None:
            fixture.write_new(baseline / "pnpm-workspace.yaml", workspace_config)
            logical_bytes += len(workspace_config)
        packages = baseline / "packages"
        packages.mkdir()
        for number in range(args.members):
            fixture.require_space(root)
            project = packages / f"member-{number:04d}"
            project.mkdir()
            manifest = json_bytes({
                "name": member_name(number), "version": "1.0.0", "private": True,
            }) + b"\n"
            fixture.write_new(project / "package.json", manifest)
            logical_bytes += len(manifest)
            (project / "node_modules").mkdir()
            members_created += 1
        payload = packages / "member-0000" / "node_modules" / "payload.bin"
        chunk = b"C" * CHUNK_BYTES
        with payload.open("xb") as stream:
            while payload_written < PAYLOAD_BYTES:
                fixture.require_space(root, PAYLOAD_BYTES - payload_written)
                stream.write(chunk)
                payload_written += len(chunk)
            stream.flush()
            os.fsync(stream.fileno())
        if payload.stat().st_blocks * 512 < PAYLOAD_BYTES:
            raise OSError("The filesystem did not allocate the full 100 MiB payload")
        logical_bytes += payload_written
        modified_ns = time.time_ns() - args.age_days * 86_400 * 1_000_000_000
        for directory, _, filenames in os.walk(baseline, topdown=False, followlinks=False):
            for filename in filenames:
                os.utime(Path(directory) / filename, ns=(modified_ns, modified_ns), follow_symlinks=False)
            os.utime(directory, ns=(modified_ns, modified_ns), follow_symlinks=False)
        fixture.require_space(root)
        marker.update(
            status="complete",
            completed_at=fixture.utc_now(),
            creation_seconds=round(time.monotonic() - started, 6),
            synthetic_modified_ns=modified_ns,
            baseline={
                "files": files,
                "directories_including_root": directories,
                "directories_excluding_root": directories - 1,
                "entries_including_root": files + directories,
                "entries_excluding_root": files + directories - 1,
                "logical_regular_file_bytes": logical_bytes,
                "symlinks": 0,
                "hardlinks": 0,
                "special_files": 0,
                "project_manifests": args.members + 1,
                "project_lockfiles": 1,
                "project_evidence_files": args.members + 2 + int(workspace_config is not None),
            },
            cases={},
        )
    except BaseException as error:
        marker.update(
            status="incomplete", workspace_members_created=members_created,
            payload_bytes_written=payload_written, error=type(error).__name__,
        )
        marker_path.write_text(json.dumps(marker, indent=2) + "\n")
        print(f"Incomplete fixture retained at {root}. Nothing was removed.", file=sys.stderr)
        raise
    marker_path.write_text(json.dumps(marker, indent=2) + "\n")
    print(json.dumps({"fixture": str(root), "baseline": str(baseline), "counts": marker["baseline"]}, indent=2))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, KeyboardInterrupt, ValueError) as error:
        print(f"Fixture creation stopped: {error}", file=sys.stderr)
        raise SystemExit(1)
