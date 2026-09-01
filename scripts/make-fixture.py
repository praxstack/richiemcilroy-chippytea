#!/usr/bin/env python3
"""Create a new, disposable chippytea benchmark fixture; never remove files."""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import shutil
import sys
import time
from datetime import datetime, timezone


MARKER = ".chippytea-benchmark-fixture.json"
MAGIC = "chippytea-disposable-benchmark-v1"
MIN_FREE_BYTES = 3 * 1024**3
MAX_PAYLOAD_BYTES = 256 * 1024**2
SOURCE_FILES_PER_DIRECTORY = 1000


def utc_now() -> str:
    return datetime.now(timezone.utc).isoformat()


def require_space(path: Path, additional_bytes: int = 0) -> None:
    free = shutil.disk_usage(path).free
    required = MIN_FREE_BYTES + additional_bytes
    if free < required:
        raise OSError(
            f"Insufficient free space: {free:,} bytes available; {required:,} required "
            "including the 3 GiB reserve. No existing files will be removed."
        )


def write_new(path: Path, data: bytes) -> None:
    with path.open("xb") as stream:
        stream.write(data)


def make_cases(root: Path) -> dict:
    cases = root / "cases"
    cases.mkdir()
    hardlinks = cases / "hardlinks"
    hardlinks.mkdir()
    write_new(hardlinks / "original.bin", b"H" * 4096)
    os.link(hardlinks / "original.bin", hardlinks / "alias.bin")

    symlinks = cases / "symlinks"
    symlinks.mkdir()
    write_new(symlinks / "target.bin", b"L" * 4096)
    os.symlink("target.bin", symlinks / "file-link")
    os.symlink("missing.bin", symlinks / "dangling-link")
    os.symlink("../hardlinks", symlinks / "directory-link")
    os.symlink(".", symlinks / "loop-link")

    sparse = cases / "sparse"
    sparse.mkdir()
    sparse_size = 64 * 1024**2
    with (sparse / "hole.bin").open("xb") as stream:
        stream.truncate(sparse_size)
    write_new(sparse / "control.bin", b"S" * 4096)
    return {
        "included_in_baseline": False,
        "hardlinks": {"regular_paths": 2, "unique_regular_inodes": 1},
        "symlinks": {"regular_files": 1, "symlinks": 4, "follow_links": False},
        "sparse": {
            "regular_files": 2,
            "logical_bytes": sparse_size + 4096,
            "allocated_bytes": "filesystem-dependent; measure independently",
        },
    }


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(description=__doc__)
    result.add_argument("path", type=Path, help="A NEW directory for this fixture")
    count = result.add_mutually_exclusive_group()
    count.add_argument("--files", type=int, help="Payload-file count (default: 10000)")
    count.add_argument("--million", action="store_true", help="Create 1,000,000 zero-byte payload files")
    result.add_argument("--bytes-per-file", type=int, help="Default: 1024; million mode requires 0")
    result.add_argument("--files-per-project", type=int, default=250)
    result.add_argument("--empty-projects", type=int, default=0, help="Add 0..10,000 projects with manifests, lockfiles and empty artifacts to exercise evidence checks")
    result.add_argument("--recent-empty-projects", type=int, default=0, help="Add empty projects whose artifact directories remain recent; combined empty-project limit: 10,000")
    result.add_argument("--source-files", type=int, default=0, help="Add 0..1,000,000 empty ordinary source files outside developer artifacts")
    result.add_argument("--source-files-per-directory", type=int, default=SOURCE_FILES_PER_DIRECTORY, help="Group ordinary source files into directories of this size (default: 1000)")
    result.add_argument("--age-days", type=int, default=0, help="Set synthetic fixture modification times this many days ago (for recommendation tests)")
    result.add_argument("--no-cases", action="store_true", help="Omit the separate link/sparse cases")
    return result


def main(argv: list[str] | None = None) -> int:
    arguments = parser()
    args = arguments.parse_args(argv)
    count = 1_000_000 if args.million else (args.files if args.files is not None else 10_000)
    size = args.bytes_per_file if args.bytes_per_file is not None else (0 if args.million else 1024)
    if not 1 <= count <= 1_000_000:
        arguments.error("--files must be between 1 and 1,000,000")
    if not 1 <= args.files_per_project <= 10_000:
        arguments.error("--files-per-project must be between 1 and 10,000")
    if not 0 <= args.empty_projects <= 10_000:
        arguments.error("--empty-projects must be between 0 and 10,000")
    if not 0 <= args.recent_empty_projects <= 10_000 or args.empty_projects + args.recent_empty_projects > 10_000:
        arguments.error("Combined empty and recent-empty projects must be between 0 and 10,000")
    if not 0 <= args.source_files <= 1_000_000:
        arguments.error("--source-files must be between 0 and 1,000,000")
    if not 1 <= args.source_files_per_directory <= 10_000:
        arguments.error("--source-files-per-directory must be between 1 and 10,000")
    if size < 0 or size > 1024**2 or count * size > MAX_PAYLOAD_BYTES:
        arguments.error("Payloads must be 0..1 MiB each and at most 256 MiB in total")
    if args.million and size != 0:
        arguments.error("--million is a metadata fixture and requires zero-byte payloads")
    if not 0 <= args.age_days <= 3650:
        arguments.error("--age-days must be between 0 and 3650")

    requested = args.path.expanduser().absolute()
    # lexists catches dangling symlinks too. Never reuse even an empty directory.
    if os.path.lexists(requested):
        arguments.error(f"Refusing an existing path: {requested}")
    payload_projects = math.ceil(count / args.files_per_project)
    projects = payload_projects + args.empty_projects + args.recent_empty_projects
    source_groups = math.ceil(args.source_files / args.source_files_per_directory)
    source_directories = 1 + source_groups if args.source_files else 0
    ancestor = requested.parent
    while not ancestor.exists():
        ancestor = ancestor.parent
    projected = count * (math.ceil(size / 4096) * 4096 + 2048) + projects * 18_432
    projected += args.source_files * 2048 + source_directories * 18_432
    require_space(ancestor, projected)
    requested.parent.mkdir(parents=True, exist_ok=True)
    root = requested.parent.resolve() / requested.name
    root.mkdir(exist_ok=False)

    marker = {
        "magic": MAGIC,
        "schema_version": 1,
        "status": "creating",
        "created_at": utc_now(),
        "generator_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
        "baseline_relative_path": "baseline",
        "minimum_free_bytes": MIN_FREE_BYTES,
        "payload_files": count,
        "bytes_per_payload_file": size,
        "files_per_project": args.files_per_project,
        "empty_projects": args.empty_projects,
        "recent_empty_projects": args.recent_empty_projects,
        "synthetic_modification_age_days": args.age_days,
        "cache_state": "Unknown; creation itself populates filesystem caches.",
    }
    marker_path = root / MARKER
    if args.source_files:
        marker.update(
            source_files=args.source_files,
            source_files_per_directory=args.source_files_per_directory,
            bytes_per_source_file=0,
            source_relative_path="baseline/source",
        )
    write_new(marker_path, (json.dumps(marker, indent=2) + "\n").encode())
    started = time.monotonic()
    created = 0
    sources_created = 0
    logical_bytes = 0
    try:
        baseline = root / "baseline"
        baseline.mkdir()
        payload = b"C" * size
        for project_number in range(projects):
            require_space(root)
            project = baseline / f"project-{project_number:06d}"
            project.mkdir()
            manifest = (
                json.dumps({"name": f"chippytea-fixture-{project_number:06d}", "private": True})
                + "\n"
            ).encode()
            write_new(project / "package.json", manifest)
            logical_bytes += len(manifest)
            lockfile = (
                json.dumps({"name": f"chippytea-fixture-{project_number:06d}", "lockfileVersion": 3, "packages": {}})
                + "\n"
            ).encode()
            write_new(project / "package-lock.json", lockfile)
            logical_bytes += len(lockfile)
            artifact = project / "node_modules"
            artifact.mkdir()
            for _ in range(min(args.files_per_project, count - created)):
                write_new(artifact / f"file-{created:07d}.bin", payload)
                logical_bytes += size
                created += 1
                if created % 1000 == 0:
                    require_space(root)
            if project_number < payload_projects and (created % 25_000 == 0 or created == count):
                print(f"Created {created:,}/{count:,} payload files", file=sys.stderr, flush=True)
        if args.source_files:
            source = baseline / "source"
            source.mkdir()
            for group in range(source_groups):
                require_space(root)
                directory = source / f"group-{group:06d}"
                directory.mkdir()
                for _ in range(min(args.source_files_per_directory, args.source_files - sources_created)):
                    write_new(directory / f"source-{sources_created:07d}.rs", b"")
                    sources_created += 1
                if sources_created % 25_000 == 0 or sources_created == args.source_files:
                    print(f"Created {sources_created:,}/{args.source_files:,} source files", file=sys.stderr, flush=True)
        case_manifest = {} if args.no_cases else make_cases(root)
        if args.age_days:
            modified = time.time() - args.age_days * 86_400
            for directory, _, filenames in os.walk(baseline, topdown=False, followlinks=False):
                for filename in filenames:
                    os.utime(Path(directory) / filename, (modified, modified), follow_symlinks=False)
                os.utime(directory, (modified, modified), follow_symlinks=False)
        if args.recent_empty_projects:
            # Ownership files retain the requested age. Only each new empty
            # artifact boundary changes, isolating early timestamp rejection.
            recent_ns = time.time_ns()
            first_recent = payload_projects + args.empty_projects
            for project_number in range(first_recent, projects):
                artifact = baseline / f"project-{project_number:06d}" / "node_modules"
                os.utime(artifact, ns=(recent_ns, recent_ns), follow_symlinks=False)
            marker["recent_artifact_modified_ns"] = recent_ns
        require_space(root)
        files = count + 2 * projects + args.source_files
        directories = 1 + 2 * projects + source_directories  # Includes baseline itself.
        marker.update(
            status="complete",
            completed_at=utc_now(),
            creation_seconds=round(time.monotonic() - started, 6),
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
                "project_manifests": projects,
                "project_lockfiles": projects,
                "project_evidence_files": 2 * projects,
            },
            cases=case_manifest,
        )
    except BaseException as error:
        marker.update(status="incomplete", payload_files_created=created, error=type(error).__name__)
        if args.source_files:
            marker["source_files_created"] = sources_created
        marker_path.write_text(json.dumps(marker, indent=2) + "\n")
        print(f"Incomplete fixture retained at {root}. Nothing was removed.", file=sys.stderr)
        raise
    marker_path.write_text(json.dumps(marker, indent=2) + "\n")
    print(json.dumps({"fixture": str(root), "baseline": str(baseline), "counts": marker["baseline"]}, indent=2))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, KeyboardInterrupt) as error:
        print(f"Fixture creation stopped: {error}", file=sys.stderr)
        raise SystemExit(1)
