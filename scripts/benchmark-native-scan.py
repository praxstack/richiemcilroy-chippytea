#!/usr/bin/env python3
"""Measure one warm, visible native rescan of an audited disposable fixture.

For stack profiling, use a separate diagnostic launch with the same isolated
environment: wait for scan_protocol=1 readiness, send SIGUSR1 once, then sample
that child process while its scan is active. Discard its timing result. Never
attach a profiler to a timed comparison or to the user's running app.
"""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import math
import os
from pathlib import Path, PurePosixPath
import signal
import sqlite3
import subprocess
import sys
import time

import benchmark

# Share the resting harness's bounded reads and synthetic-library validation.
_spec = importlib.util.spec_from_file_location("chippytea_native_cpu", Path(__file__).with_name("benchmark-native-cpu.py"))
if _spec is None or _spec.loader is None:
    raise RuntimeError("The shared native benchmark helpers could not be loaded")
energy = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(energy)

SCAN_PROTOCOL = 1
LEDGER_TABLES = ("wallet", "earnings", "operations")


def declared_paths(args: argparse.Namespace, marker: dict) -> list[str]:
    paths = args.expected_eligible
    if paths is None:
        paths = marker.get("expected_eligible_relative_paths")
    if args.expect_no_findings:
        if paths:
            raise RuntimeError("Positive paths conflict with --expect-no-findings")
        return []
    if not isinstance(paths, list) or not paths or len(paths) > 256:
        raise RuntimeError("Declare expected positive paths with --expected-eligible or a fixture marker; use --expect-no-findings for an explicit negative control")
    for path in paths:
        if (not isinstance(path, str) or not path or len(path.encode()) > 1024
                or PurePosixPath(path).is_absolute() or any(part in ("", ".", "..") for part in path.split("/"))):
            raise RuntimeError("Expected candidates must be bounded paths relative to baseline")
    if len(set(paths)) != len(paths):
        raise RuntimeError("Expected candidate paths must be unique")
    return sorted(paths)


def validate_coverage(stats: dict, expected: dict, eligible_count: int) -> None:
    if (not isinstance(stats, dict) or stats.get("complete") is not True
            or stats.get("cancelled") is not False or stats.get("errors") != 0):
        raise RuntimeError(f"Native scan did not complete safely: {stats}")
    for name, key in (("entries", "entries_including_root"), ("files", "files"),
                      ("directories", "directories_including_root")):
        if stats.get(name) != expected[key]:
            raise RuntimeError(f"Native {name} coverage differs: {stats.get(name)} != {expected[key]}")
    if stats.get("candidates") != eligible_count or (stats.get("firstFindingMs") is not None) != (eligible_count > 0):
        raise RuntimeError("Fresh scan candidate counts do not match the declared findings")


def validate_state(record: dict, process: subprocess.Popen, root: Path, coins: int, reduced: bool, phase: str) -> None:
    if (record.get("scan_protocol") != SCAN_PROTOCOL or record.get("pid") != process.pid
            or record.get("phase") != phase or record.get("destination") != "Find space"
            or record.get("root_path") != str(root) or record.get("visible") is not True
            or record.get("panel_visible") is not True or record.get("occluded") is not False
            or record.get("scanning") is not False or record.get("cleaning") is not False
            or record.get("collected_coins") != coins or record.get("pending_coins") != 0
            or record.get("reduce_motion") is not reduced or record.get("effective_reduce_motion") is not reduced):
        raise RuntimeError(f"Invalid native scan state: {record}")


def wait_output(process: subprocess.Popen, output: Path, name: str, deadline: float) -> dict:
    while not (output / name).exists():
        if name != "final.json" and (output / "final.json").exists():
            raise RuntimeError(f"Native setup failed: {energy.bounded_json(output / 'final.json')}")
        if process.poll() is not None or time.monotonic() >= deadline:
            raise RuntimeError("Native scan benchmark did not complete; inspect process.log")
        time.sleep(0.05)
    return energy.bounded_json(output / name)


def ledger(database: Path) -> dict:
    state = energy.library_state(database)
    return {name: state[name] for name in LEDGER_TABLES}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("fixture", type=Path)
    parser.add_argument("--app", type=Path, default=benchmark.REPO / "build/Chippytea.app")
    parser.add_argument("--output", type=Path, required=True, help="New directory under benchmarks/local, outside the fixture")
    parser.add_argument("--expected-eligible", action="append", help="Exact positive path relative to baseline; repeat for each expected suggestion")
    parser.add_argument("--expect-no-findings", action="store_true", help="Declare a negative control; no first-finding claim will be made")
    parser.add_argument("--coins", type=int, default=0, help="Synthetic collected balance; must match a reused harness state")
    parser.add_argument("--reduce-motion", action="store_true")
    args = parser.parse_args()
    if not 0 <= args.coins <= 1_000_000 or args.fixture.is_symlink():
        parser.error("Use 0..1000000 synthetic coins and a physical fixture")
    fixture = args.fixture.resolve(strict=True)
    marker = energy.bounded_json(fixture / benchmark.MARKER)
    root = fixture / "baseline"
    if (marker.get("magic") != benchmark.MAGIC or marker.get("status") != "complete"
            or marker.get("baseline_relative_path") != "baseline" or root.is_symlink() or not root.is_dir()):
        parser.error("Use a complete marked disposable fixture")
    expected = marker.get("baseline", {})
    if any(type(expected.get(key)) is not int or expected[key] < 0 for key in
           ("files", "directories_including_root", "entries_including_root")):
        parser.error("The marker must declare exact file, directory and entry counts")
    positive_paths = declared_paths(args, marker)
    output = args.output.resolve()
    if benchmark.REPO / "benchmarks/local" not in output.parents or fixture == output or fixture in output.parents:
        parser.error("Output must be a fresh benchmarks/local directory outside the fixture")
    output.mkdir(parents=True, exist_ok=False)
    app = args.app.resolve(strict=True)
    binary = app / "Contents/MacOS/Chippytea"
    binary_hash = hashlib.sha256(binary.read_bytes()).hexdigest()
    state = fixture / "state"
    state_marker = fixture / ".chippytea-energy-state.json"
    expected_state_marker = {"magic": "chippytea-synthetic-energy-state-v1", "coins": args.coins}
    if state.exists() or state.is_symlink():
        if energy.bounded_json(state_marker) != expected_state_marker:
            raise RuntimeError("Existing state is not this synthetic wallet; use a fresh fixture")
        energy.validate_state(state, root, args.coins)
    elif state_marker.exists() or state_marker.is_symlink():
        raise RuntimeError("A state marker exists without its database; use a fresh fixture")
    audit_before = benchmark.verify_fixture(root, expected)
    audit_before["performed"] = "before setup and the timed scan; warms filesystem caches"
    if not audit_before["matches_marker"]:
        raise RuntimeError("Fixture failed its independent pre-scan audit")
    environment = dict(os.environ)
    environment.update(CHIPPYTEA_DATA_DIR=str(state), CHIPPYTEA_SCREENSHOT_ROOT=str(root),
                       CHIPPYTEA_SCREENSHOT_STATE="discover", CHIPPYTEA_SCREENSHOT=str(output / "window.png"),
                       CHIPPYTEA_ENERGY_OUTPUT=str(output / "ready.json"))
    preferences = ["-reduceMotion", "YES" if args.reduce_motion else "NO"]
    database = state / "library.sqlite"
    if not state.exists():
        state.mkdir(mode=0o700)
        with state_marker.open("x") as stream:
            stream.write(json.dumps(expected_state_marker) + "\n")
        with (output / "stage.log").open("x") as log:
            subprocess.run([str(binary), "--screenshot", *preferences], env=environment,
                           stdout=log, stderr=subprocess.STDOUT, timeout=90, check=True)
        energy.validate_state(state, root, None)
        # This library was just created from the disposable fixture. No real
        # wallet, receipt or root is copied into a measurement process.
        with sqlite3.connect(database) as connection:
            connection.execute("UPDATE wallet SET collected=?,remainder=50000000,credited=? WHERE id=1",
                               (args.coins, args.coins * 100_000_000 + 50_000_000))
        energy.validate_state(state, root, args.coins)
    before_ledger = ledger(database)
    command = [str(binary), "--scan-benchmark", *preferences]
    report = None
    try:
        with (output / "process.log").open("x") as log:
            process = subprocess.Popen(command, env=environment, stdout=log,
                                       stderr=subprocess.STDOUT, start_new_session=True)
            try:
                ready = wait_output(process, output, "ready.json", time.monotonic() + 70)
                validate_state(ready, process, root, args.coins, args.reduce_motion, "ready")
                validate_coverage(ready.get("setup_stats"), expected, len(positive_paths))
                if ready.get("cached_eligible_paths") != positive_paths:
                    raise RuntimeError("Untimed setup findings do not match the declared fixture positives")
                time.sleep(3)  # Entrance settles before the native clock starts.
                if process.poll() is not None:
                    raise RuntimeError("Native benchmark exited before its start handshake")
                # Only the advertised scan protocol installs this handler.
                os.kill(process.pid, signal.SIGUSR1)
                final = wait_output(process, output, "final.json", time.monotonic() + 65)
                validate_state(final, process, root, args.coins, args.reduce_motion, "complete")
                validate_coverage(final.get("stats"), expected, len(positive_paths))
                if (final.get("valid") is not True or final.get("observed_active_edge") is not True
                        or final.get("cached_eligible_paths") != positive_paths
                        or final.get("final_eligible_paths") != positive_paths
                        or final.get("new_eligible_paths") != [] or final.get("first_new_path_observed_ms") is not None):
                    raise RuntimeError("Measured scan findings or active edges failed validation")
                if positive_paths and (final.get("first_revalidated_finding_engine_ms") is None
                                       or final.get("first_revalidated_finding_observed_ms") is None):
                    raise RuntimeError("Cached rows cannot substitute for a fresh finding in the measured scan")
                for key in ("wall_seconds", "user_cpu_seconds", "system_cpu_seconds", "cpu_seconds", "cpu_percent_of_one_core"):
                    value = final.get(key)
                    if type(value) not in (int, float) or not math.isfinite(value) or value < 0:
                        raise RuntimeError(f"Invalid native timing field: {key}")
                if final["wall_seconds"] <= 0 or final["wall_seconds"] > 60:
                    raise RuntimeError("Measured scan duration is outside the declared deadline")
                energy.validate_state(state, root, args.coins)
                if ledger(database) != before_ledger:
                    raise RuntimeError("Wallet or cleanup history changed during the native scan")
                if hashlib.sha256(binary.read_bytes()).hexdigest() != binary_hash:
                    raise RuntimeError("App binary changed during the native scan")
                report = {"hardware": benchmark.hardware(), "command": command, "app_sha256": binary_hash,
                          "harness_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
                          "shared_harness_sha256": hashlib.sha256(Path(energy.__file__).read_bytes()).hexdigest(),
                          "fixture": str(fixture), "expected_eligible_paths": positive_paths,
                          "synthetic_wallet": True, "ledger_unchanged": True, "ledger_tables_checked": list(LEDGER_TABLES),
                          "ready": ready, "native": final, "audit_before": audit_before,
                          "coverage_note": "Exact file, directory and entry counts; Suggestions mode does not promise full metadata bytes for ordinary source files.",
                          "method": "One warm visible native rescan after an untimed setup scan in the same process. Uses ordinary Scan action and normal snapshot delivery. Native CPU/wall includes bridge and observer delivery, not compositor latency. Not CLI traversal or resting CPU. No profiler attached; no cleanup action."}
            finally:
                benchmark.stop_process(process)
    finally:
        audit_after = benchmark.verify_fixture(root, expected)
        (output / "audit-after.json").write_text(json.dumps(audit_after, indent=2) + "\n")
    if not audit_after["matches_marker"]:
        raise RuntimeError("Fixture failed its final independent audit")
    if ledger(database) != before_ledger:
        raise RuntimeError("Wallet or cleanup history changed at process shutdown")
    report["audit_after"] = audit_after
    (output / "summary.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({key: report["native"][key] for key in ("wall_seconds", "cpu_seconds", "cpu_percent_of_one_core")}))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
