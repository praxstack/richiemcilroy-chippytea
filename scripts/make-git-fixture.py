#!/usr/bin/env python3
"""Create a disposable four-artifact Git tracking fixture; never remove files.

Uses 400 MiB of independently written payloads and requires a 16 GiB reserve.
Only a NEW chippytea-git-* directory directly under /private/tmp is allowed.
Three artifacts are untracked; the fourth contains synthetic tracked source.
"""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import stat
import subprocess
import time

BASE = Path(__file__).resolve().parent
WORKSPACE_SCRIPT = BASE / "make-workspace-fixture.py"
HELPER_SCRIPT = WORKSPACE_SCRIPT.with_name("make-fixture.py")
PAYLOAD_BYTES = 100 * 1024**2
FREE_FLOOR = 16 * 1024**3

def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()

def admit(path, additional_bytes=0):
    if shutil.disk_usage(path).free < FREE_FLOOR + additional_bytes:
        raise OSError("Insufficient free space for this bounded fixture")

def raise_walk_error(error):
    raise error

def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("path", type=Path)
    args = parser.parse_args(argv)
    requested = args.path.expanduser().absolute()
    if os.path.lexists(requested):
        parser.error("Refusing an existing fixture path")
    approved_parent = requested.parent.resolve()
    if approved_parent != Path("/private/tmp") or not requested.name.startswith("chippytea-git-"):
        parser.error("Use a new /private/tmp/chippytea-git-... directory")
    # Use the approved physical parent, not a live alias through another path.
    requested = approved_parent / requested.name
    if os.path.lexists(requested):
        parser.error("Refusing an existing fixture path")
    admit(requested.parent, 4 * PAYLOAD_BYTES + 8 * 1024**2)
    spec = importlib.util.spec_from_file_location("git_ready_workspace_fixture", WORKSPACE_SCRIPT)
    if spec is None or spec.loader is None:
        raise RuntimeError("Cannot load the adjacent workspace generator")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    # The isolated helper module must use the same floor for the initial
    # workspace's payload writes and marker, not its ordinary 3 GiB default.
    module.fixture.MIN_FREE_BYTES = FREE_FLOOR
    module.fixture.require_space = admit
    started = time.monotonic()
    result = module.main([str(requested), "--lock-format", "pnpm", "--members", "4", "--lock-kib", "256", "--age-days", "9"])
    if result != 0:
        raise RuntimeError("Initial workspace creation failed")
    root = requested.resolve()
    baseline = root / "baseline"
    marker_path = root / module.fixture.MARKER
    marker = json.loads(marker_path.read_text())
    original = dict(marker["baseline"])
    marker.update(status="extending_with_git", original_generator_sha256=marker["generator_sha256"],
                  generator_sha256=sha(__file__), original_workspace_creation_seconds=marker.pop("creation_seconds"))
    marker_path.write_text(json.dumps(marker, indent=2) + "\n")
    chunk = b"C" * 1024**2
    extra_payload = 0
    try:
        for member in range(1, 4):
            path = baseline / f"packages/member-{member:04d}/node_modules/payload.bin"
            with path.open("xb") as stream:
                remaining = PAYLOAD_BYTES
                while remaining:
                    admit(root, remaining)
                    stream.write(chunk)
                    remaining -= len(chunk)
                stream.flush()
                os.fsync(stream.fileno())
            meta = path.lstat()
            if not (stat.S_ISREG(meta.st_mode) and meta.st_size == PAYLOAD_BYTES
                    and meta.st_blocks * 512 >= PAYLOAD_BYTES and meta.st_nlink == 1):
                raise RuntimeError("Payload is not an independently allocated 100 MiB regular file")
            extra_payload += PAYLOAD_BYTES
        tracked_text = b"Preserve this synthetic tracked source.\n"
        tracked = ["tracked-source.txt", "packages/member-0003/node_modules/tracked-source.txt"]
        for name in tracked:
            with (baseline / name).open("xb") as stream:
                stream.write(tracked_text)
        env = {"PATH": "/usr/bin:/bin", "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": "/dev/null", "GIT_TERMINAL_PROMPT": "0"}
        git_commands = [["init", "--quiet", "--template="], ["add", "--", *tracked]]
        for args in git_commands:
            completed = subprocess.run(["/usr/bin/git", "-C", str(baseline), *args], env=env, capture_output=True, text=True, timeout=20)
            if completed.returncode:
                raise RuntimeError("Synthetic Git preparation failed: " + completed.stderr)
        git_files = {}
        git_dirs = []
        for directory, directories, files in os.walk(baseline / ".git", followlinks=False, onerror=raise_walk_error):
            path = Path(directory)
            git_dirs.append(str(path.relative_to(baseline)))
            if not stat.S_ISDIR(path.lstat().st_mode):
                raise RuntimeError("Synthetic Git directory changed type")
            for name in directories:
                if not stat.S_ISDIR((path / name).lstat().st_mode):
                    raise RuntimeError("Synthetic Git child is not a real directory")
            for name in files:
                file = path / name
                meta = file.lstat()
                if not (stat.S_ISREG(meta.st_mode) and meta.st_nlink == 1):
                    raise RuntimeError("Synthetic Git file is not an independent regular file")
                git_files[str(file.relative_to(baseline))] = {"bytes": meta.st_size, "sha256": sha(file)}
        modified_ns = time.time_ns() - 9 * 86_400 * 1_000_000_000
        for directory, _, filenames in os.walk(baseline, topdown=False, followlinks=False, onerror=raise_walk_error):
            for name in filenames:
                os.utime(Path(directory) / name, ns=(modified_ns, modified_ns), follow_symlinks=False)
            os.utime(directory, ns=(modified_ns, modified_ns), follow_symlinks=False)
        files = original["files"] + 3 + len(tracked) + len(git_files)
        directories = original["directories_including_root"] + len(git_dirs)
        marker.update(
            status="complete", fixture_kind="synthetic-git-tracking-four-artifacts",
            completed_at=module.fixture.utc_now(), minimum_free_bytes=FREE_FLOOR,
            synthetic_modified_ns=modified_ns, payload_files=4, empty_artifacts=0,
            expected_eligible_relative_paths=[f"packages/member-{n:04d}/node_modules" for n in range(3)],
            expected_tracked_diagnostic="packages/member-0003/node_modules",
            git_commands=git_commands, git_tracked_paths=tracked,
            git_files=git_files, git_directories=sorted(git_dirs),
            coverage_note="The physical audit includes .git; Suggestions intentionally skips its contents. Compare complete non-timing engine stats across variants, not engine entry totals to physical .git inventory.",
            baseline={**original, "files": files, "directories_including_root": directories,
                      "directories_excluding_root": directories - 1, "entries_including_root": files + directories,
                      "entries_excluding_root": files + directories - 1,
                      "logical_regular_file_bytes": original["logical_regular_file_bytes"] + extra_payload + len(tracked) * len(tracked_text) + sum(item["bytes"] for item in git_files.values())},
        )
        admit(root)
    except BaseException as error:
        marker.update(status="incomplete", error=type(error).__name__, extra_payload_bytes_written=extra_payload)
        raise
    finally:
        marker["creation_seconds"] = round(time.monotonic() - started, 6)
        marker_path.write_text(json.dumps(marker, indent=2) + "\n")
    print(json.dumps({"fixture": str(root), "marker_sha256": sha(marker_path), "physical_counts": marker["baseline"], "expected_eligible": marker["expected_eligible_relative_paths"]}, indent=2))
    return 0

if __name__ == "__main__":
    raise SystemExit(main())
