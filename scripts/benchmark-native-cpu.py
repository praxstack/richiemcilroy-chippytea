#!/usr/bin/env python3
"""Measure a settled visible native window using an isolated, synthetic wallet."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import signal
import sqlite3
import stat
import subprocess
import time

import benchmark

ENERGY_PROTOCOL = 2
PAGE_TITLES = {"coins": "Your chips", "discover": "Find space", "settings": "Settings"}
STATE_FILES = {"bookmarks.json", "library.sqlite", "library.sqlite-wal", "library.sqlite-shm", "library.lock"}


def physical_file(path: Path, maximum_bytes: int = 64 * 1024 * 1024) -> int:
    descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC | os.O_NONBLOCK)
    try:
        info = os.fstat(descriptor)
        if (not stat.S_ISREG(info.st_mode) or info.st_uid != os.geteuid()
                or info.st_nlink != 1 or not 0 <= info.st_size <= maximum_bytes
                or getattr(info, "st_flags", 0) & 0x40000000):
            raise RuntimeError(f"Expected a bounded, physical, owned fixture file: {path}")
    except BaseException:
        os.close(descriptor)
        raise
    return descriptor


def bounded_json(path: Path) -> dict:
    with os.fdopen(physical_file(path, 65536), "rb") as stream:
        content = stream.read(65537)
    if len(content) > 65536:
        raise RuntimeError(f"Fixture JSON grew beyond its bound: {path}")
    value = json.loads(content)
    if not isinstance(value, dict):
        raise RuntimeError(f"Expected a JSON object: {path}")
    return value


def validate_state(state: Path, root: Path, coins: int | None) -> None:
    if state.is_symlink() or not state.is_dir() or state.resolve(strict=True) != state:
        raise RuntimeError("Benchmark state must remain a physical directory")
    with os.scandir(state) as entries:
        for entry in entries:
            if entry.name not in STATE_FILES:
                raise RuntimeError(f"Unexpected benchmark state file: {entry.name}")
            os.close(physical_file(Path(entry.path)))
    if set(bounded_json(state / "bookmarks.json")) != {str(root)}:
        raise RuntimeError("Benchmark bookmarks must refer only to its disposable baseline")
    database = state / "library.sqlite"
    with sqlite3.connect(database.as_uri() + "?mode=ro", uri=True) as connection:
        roots = connection.execute("SELECT path,json FROM roots").fetchall()
        if (len(roots) != 1 or roots[0][0] != str(root)
                or json.loads(roots[0][1]).get("path") != str(root)
                or json.loads(roots[0][1]).get("kind") != "folder"):
            raise RuntimeError("Benchmark library contains an unexpected scan root")
        for table in ("operations", "earnings", "pending_scopes", "active_scopes", "refreshes"):
            if connection.execute(f"SELECT count(*) FROM {table}").fetchone()[0]:
                raise RuntimeError(f"Benchmark library is not settled and synthetic: {table}")
        expected = (1, 0, 0, 0) if coins is None else (1, coins, 50_000_000, coins * 100_000_000 + 50_000_000)
        if connection.execute("SELECT id,collected,remainder,credited FROM wallet").fetchall() != [expected]:
            raise RuntimeError("Benchmark wallet does not match the synthetic fixture")


def validate_native_state(value: dict, pid: int, page: str, coins: int, reduced: bool, final: bool) -> None:
    if (value.get("energy_protocol") != ENERGY_PROTOCOL or value.get("pid") != pid
            or value.get("final") is not final
            or value.get("visible_windows") != 1 or value.get("panel_visible") is not True
            or value.get("occluded") is not False or value.get("scanning") is not False
            or value.get("cleaning") is not False or value.get("destination") != PAGE_TITLES[page]
            or any(value.get(key) is not False for key in ("busy", "review_open", "access_setup_open",
                                                         "collecting", "system_dialog_open", "has_error"))
            or value.get("collected_coins") != coins or value.get("reduce_motion") is not reduced
            or value.get("effective_reduce_motion") is not reduced or value.get("invalidations") != []):
        raise RuntimeError(f"Unexpected or invalidated native measurement state: {value}")


def process_usage(pid: int) -> tuple[float, int]:
    fields = subprocess.check_output(["ps", "-p", str(pid), "-o", "time=,rss="], text=True).split()
    seconds = sum(float(part) * 60**index for index, part in enumerate(reversed(fields[0].split(":"))))
    return seconds, int(fields[1]) * 1024


def library_state(database: Path) -> dict:
    with sqlite3.connect(database.as_uri() + "?mode=ro", uri=True) as connection:
        return {table: connection.execute(f"SELECT * FROM {table} ORDER BY rowid").fetchall()
                for table in ("wallet", "earnings", "operations", "scans", "pending_scopes", "active_scopes")}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("fixture", type=Path, help="Complete disposable fixture; its sibling state must belong to this harness")
    parser.add_argument("--app", type=Path, default=benchmark.REPO / "build/Chippytea.app")
    parser.add_argument("--page", choices=("coins", "discover", "settings"), default="coins")
    parser.add_argument("--coins", type=int, default=118, help="Synthetic collected balance in the disposable database only")
    parser.add_argument("--seconds", type=float, default=20)
    parser.add_argument("--reduce-motion", action="store_true")
    parser.add_argument("--output", type=Path, required=True, help="New directory under benchmarks/local")
    args = parser.parse_args()
    if not 5 <= args.seconds <= 60 or not 0 <= args.coins <= 1_000_000:
        parser.error("Use 5..60 seconds and 0..1000000 synthetic coins")
    if args.fixture.is_symlink():
        parser.error("Fixture must be a physical directory")
    fixture = args.fixture.resolve(strict=True)
    marker_path = fixture / benchmark.MARKER
    if marker_path.is_symlink() or not marker_path.is_file() or marker_path.stat().st_size > 65536:
        parser.error("A bounded regular fixture marker is required")
    marker = bounded_json(marker_path)
    root = fixture / "baseline"
    if (marker.get("magic") != benchmark.MAGIC or marker.get("status") != "complete"
            or marker.get("baseline_relative_path") != "baseline" or root.is_symlink()):
        parser.error("Use a complete marked Chippytea fixture")
    state = fixture / "state"
    state_marker = fixture / ".chippytea-energy-state.json"
    expected_state_marker = {"magic": "chippytea-synthetic-energy-state-v1", "coins": args.coins}
    if state.exists() or state.is_symlink():
        if state.is_symlink() or state_marker.is_symlink() or not state_marker.is_file():
            parser.error("Existing state was not created by this energy harness")
        if bounded_json(state_marker) != expected_state_marker:
            parser.error("Use the same synthetic balance or create a new fixture")
        validate_state(state, root, args.coins)
    elif state_marker.exists() or state_marker.is_symlink():
        parser.error("State marker exists without its database; create a new fixture")
    output = args.output.resolve()
    if benchmark.REPO / "benchmarks/local" not in output.parents or fixture in output.parents:
        parser.error("Output must be a new benchmarks/local directory outside the fixture")
    output.mkdir(parents=True, exist_ok=False)
    app = args.app.resolve(strict=True)
    binary = app / "Contents/MacOS/Chippytea"
    binary_hash = hashlib.sha256(binary.read_bytes()).hexdigest()
    audit_before = benchmark.verify_fixture(root, marker["baseline"])
    if not audit_before["matches_marker"]:
        raise RuntimeError("Fixture failed its independent audit")
    environment = dict(os.environ)
    environment.update(CHIPPYTEA_DATA_DIR=str(state), CHIPPYTEA_SCREENSHOT_ROOT=str(root),
                       CHIPPYTEA_SCREENSHOT_STATE=args.page, CHIPPYTEA_SCREENSHOT=str(output / "window.png"),
                       CHIPPYTEA_ENERGY_OUTPUT=str(output / "ready.json"))
    preferences = ["-reduceMotion", "YES" if args.reduce_motion else "NO"]
    database = state / "library.sqlite"
    if not state.exists():
        state.mkdir(mode=0o700)
        with state_marker.open("x") as stream:
            stream.write(json.dumps(expected_state_marker) + "\n")
        with (output / "stage.log").open("w") as log:
            subprocess.run([str(binary), "--screenshot", *preferences], env=environment,
                           stdout=log, stderr=subprocess.STDOUT, timeout=90, check=True)
        # This is a newly created, marked benchmark database. No real wallet or
        # receipt is copied, credited, collected or modified by the harness.
        validate_state(state, root, None)
        with sqlite3.connect(database) as connection:
            connection.execute("UPDATE wallet SET collected=?,remainder=50000000,credited=? WHERE id=1",
                               (args.coins, args.coins * 100_000_000 + 50_000_000))
        validate_state(state, root, args.coins)
    command = [str(binary), "--energy-benchmark", *preferences]
    with (output / "process.log").open("w") as log:
        process = subprocess.Popen(command, env=environment, stdout=log,
                                   stderr=subprocess.STDOUT, start_new_session=True)
        try:
            deadline = time.monotonic() + 90
            while not (output / "ready.json").exists():
                if process.poll() is not None or time.monotonic() >= deadline:
                    raise RuntimeError("Native window did not become ready; inspect process.log")
                time.sleep(0.1)
            ready = bounded_json(output / "ready.json")
            # Older binaries never receive SIGUSR1: only protocol 2 advertises
            # the installed one-shot query handler and event-driven invalidation.
            validate_native_state(ready, process.pid, args.page, args.coins, args.reduce_motion, False)
            time.sleep(3)
            before_state = library_state(database)
            before_cpu, before_rss = process_usage(process.pid)
            started = time.monotonic()
            time.sleep(args.seconds)
            after_cpu, after_rss = process_usage(process.pid)
            elapsed = time.monotonic() - started
            if process.poll() is not None:
                raise RuntimeError("Native measurement process exited before its final query")
            os.kill(process.pid, signal.SIGUSR1)
            deadline = time.monotonic() + 5
            while not (output / "final.json").exists():
                if process.poll() is not None or time.monotonic() >= deadline:
                    raise RuntimeError("Native process did not answer its final state query")
                time.sleep(0.05)
            final_state = bounded_json(output / "final.json")
            validate_native_state(final_state, process.pid, args.page, args.coins, args.reduce_motion, True)
            after_state = library_state(database)
            if before_state != after_state:
                raise RuntimeError("Library changed during the settled measurement")
            if any(after_state[table] for table in ("pending_scopes", "active_scopes")):
                raise RuntimeError("Scan work remained pending during measurement")
            if binary_hash != hashlib.sha256(binary.read_bytes()).hexdigest():
                raise RuntimeError("App binary changed during measurement")
            report = {"hardware": benchmark.hardware(), "command": command, "app_sha256": binary_hash,
                      "harness_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
                      "fixture": str(fixture), "synthetic_wallet": True, "ready": ready, "final": final_state,
                      "elapsed_seconds": elapsed, "cpu_seconds": after_cpu - before_cpu,
                      "cpu_percent_of_one_core": (after_cpu - before_cpu) / elapsed * 100,
                      "resident_bytes_before": before_rss, "resident_bytes_after": after_rss,
                      "library_unchanged": True, "library_tables_checked": sorted(before_state), "audit_before": audit_before,
                      "method": "Settled visible native window, three-second settling interval, ps process CPU time at 10 ms resolution. RSS is sampled, not peak. Protocol 2 records event-driven invalidation and a one-shot final state query outside timing. No active scan or mutation; synthetic wallet in isolated state. Excludes startup and collection animation."}
            (output / "summary.json").write_text(json.dumps(report, indent=2) + "\n")
            print(json.dumps({key: report[key] for key in ("elapsed_seconds", "cpu_seconds", "cpu_percent_of_one_core")}))
        finally:
            benchmark.stop_process(process)
    audit_after = benchmark.verify_fixture(root, marker["baseline"])
    report["audit_after"] = audit_after
    (output / "summary.json").write_text(json.dumps(report, indent=2) + "\n")
    if not audit_after["matches_marker"]:
        raise RuntimeError("Fixture failed its final audit")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
