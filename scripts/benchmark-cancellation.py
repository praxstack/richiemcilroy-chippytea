#!/usr/bin/env python3
"""Measure cancellation of a release CLI scan on a marked disposable fixture.

Only summary statistics are persisted. Raw child stdout/stderr and discovered
paths are not written to disk. The fixture is never changed or removed.
"""

from __future__ import annotations

import argparse
from datetime import datetime, timezone
import hashlib
import importlib.util
import json
import math
import os
from pathlib import Path
import platform
import signal
import statistics
import subprocess
import sys
import time


REPO = Path(__file__).resolve().parents[1]
MARKER = ".chippytea-benchmark-fixture.json"
MAGIC = "chippytea-disposable-benchmark-v1"
STAT_FIELDS = (
    "entries", "files", "directories", "logical_bytes", "allocated_bytes",
    "skipped", "errors", "candidates", "elapsed_ms", "first_finding_ms",
    "cancelled", "complete",
)


def utc_now() -> str:
    return datetime.now(timezone.utc).isoformat()


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def benchmark_helpers():
    specification = importlib.util.spec_from_file_location(
        "chippytea_benchmark_helpers", Path(__file__).with_name("benchmark.py")
    )
    if specification is None or specification.loader is None:
        raise RuntimeError("The adjacent benchmark.py helper could not be loaded")
    module = importlib.util.module_from_spec(specification)
    previous = sys.dont_write_bytecode
    try:
        sys.dont_write_bytecode = True
        specification.loader.exec_module(module)
    finally:
        sys.dont_write_bytecode = previous
    return module


def terminate(process: subprocess.Popen) -> tuple[bytes, bytes]:
    if process.poll() is None:
        try:
            os.killpg(process.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
    try:
        return process.communicate(timeout=2)
    except subprocess.TimeoutExpired:
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        return process.communicate(timeout=2)


def final_stats(stdout: bytes) -> dict | None:
    for line in reversed(stdout.splitlines()):
        try:
            value = json.loads(line)
        except (ValueError, UnicodeError):
            continue
        if not isinstance(value, dict):
            continue
        stats = value.get("stats", value)
        if isinstance(stats, dict) and all(key in stats for key in ("entries", "cancelled", "complete", "errors")):
            return {key: stats[key] for key in STAT_FIELDS if key in stats}
    return None


def sample(command: list[str], number: int, delay_ms: int, timeout: float) -> dict:
    started = time.perf_counter()
    process = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE, start_new_session=True)
    timed_out = False
    try:
        try:
            stdout, stderr = process.communicate(timeout=timeout)
        except subprocess.TimeoutExpired:
            timed_out = True
            stdout, stderr = terminate(process)
    except BaseException:
        terminate(process)
        raise
    wall_ms = (time.perf_counter() - started) * 1000
    stats = final_stats(stdout)
    failures = []
    if timed_out:
        failures.append("process timeout")
    if process.returncode != 0:
        failures.append(f"nonzero exit status: {process.returncode}")
    if stats is None:
        failures.append("no final scan statistics")
    else:
        if stats.get("cancelled") is not True:
            failures.append("cancelled was not true; the fixture may finish before the requested delay")
        if stats.get("complete") is not False:
            failures.append("complete was not false")
        if type(stats.get("errors")) is not int or stats["errors"] != 0:
            failures.append("scan reported errors or an invalid error count")
    return {
        "sample": number,
        "requested_cancel_after_ms": delay_ms,
        "process_wall_ms": round(wall_ms, 3),
        "wall_minus_delay_upper_bound_ms": round(max(0, wall_ms - delay_ms), 3),
        "exit_code": process.returncode,
        "timed_out": timed_out,
        "stats": stats,
        "stdout_bytes": len(stdout),
        "stderr_bytes": len(stderr),
        "failures": failures,
        "passed": not failures,
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("fixture", type=Path, help="Complete marked fixture root; baseline/ must outlast the cancellation delay")
    parser.add_argument("--cli", type=Path, default=REPO / "target/release/chippytea-cli", help="Final release CLI to measure")
    parser.add_argument("--samples", type=int, default=20, help="Sequential samples (default: 20)")
    parser.add_argument("--mode", choices=("metadata-coverage", "suggestions"), default="metadata-coverage", help="Traversal policy (default: metadata-coverage)")
    parser.add_argument("--cancel-after-ms", type=int, default=40, help="Requested in-process cancellation delay (default: 40)")
    parser.add_argument("--timeout-seconds", type=float, default=10, help="Timeout for each process (default: 10)")
    parser.add_argument("--target-ms", type=float, default=200, help="Report whether every measured upper bound meets this target (default: 200)")
    parser.add_argument("--output", type=Path, help="New results directory; defaults to benchmarks/local/cancellation-<timestamp>")
    args = parser.parse_args(argv)
    if platform.system() != "Darwin":
        parser.error("This native release benchmark requires macOS")
    if not 1 <= args.samples <= 200 or not 1 <= args.cancel_after_ms <= 10_000:
        parser.error("Use 1..200 samples and a 1..10000 ms cancellation delay")
    if not math.isfinite(args.timeout_seconds) or not args.cancel_after_ms / 1000 + 1 <= args.timeout_seconds <= 120:
        parser.error("Timeout must allow at least one second beyond the delay and be at most 120 seconds")
    if not math.isfinite(args.target_ms) or args.target_ms <= 0:
        parser.error("The reported target must be a positive finite number")
    requested = args.fixture.expanduser()
    if requested.is_symlink():
        parser.error("Fixture root must not be a symlink")
    fixture = requested.resolve(strict=True)
    marker_path = fixture / MARKER
    if marker_path.is_symlink() or not marker_path.is_file() or marker_path.stat().st_size > 1024**2:
        parser.error("A regular Chippytea fixture marker is required")
    marker = json.loads(marker_path.read_text())
    if not isinstance(marker, dict) or marker.get("magic") != MAGIC or marker.get("status") != "complete" or marker.get("baseline_relative_path") != "baseline":
        parser.error("A complete marked Chippytea disposable fixture is required")
    baseline = fixture / "baseline"
    if baseline.is_symlink() or not baseline.is_dir():
        parser.error("Fixture baseline must be a physical directory")
    cli = args.cli.expanduser().resolve(strict=True)
    if not cli.is_file() or not os.access(cli, os.X_OK):
        parser.error("Build the final release CLI before measuring cancellation")
    timestamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%S.%fZ")
    output = (args.output or REPO / "benchmarks/local" / f"cancellation-{timestamp}").expanduser().resolve()
    if output == fixture or fixture in output.parents:
        parser.error("Results must be outside the fixture")
    output.mkdir(parents=True, exist_ok=False)
    helper = benchmark_helpers()
    command = [str(cli), "scan", str(baseline), "--cancel-after-ms", str(args.cancel_after_ms)]
    if args.mode == "metadata-coverage":
        command.append("--metadata-coverage")
    report = {
        "schema_version": 1,
        "status": "running",
        "started_at": utc_now(),
        "hardware": helper.hardware(),
        "command": command,
        "mode": args.mode,
        "harness_sha256": sha256(Path(__file__)),
        "cli_binary_sha256": sha256(cli),
        "git_head": helper.command_text(["git", "-C", str(REPO), "rev-parse", "HEAD"]),
        "fixture_marker_sha256": sha256(marker_path),
        "fixture_baseline_counts": marker.get("baseline", {}),
        "requested_samples": args.samples,
        "target_upper_bound_ms": args.target_ms,
        "method": "Sequential whole-process measurements. Wall time minus the requested delay is an upper bound including startup, timer scheduling and shutdown; it does not timestamp the Rust cancellation flag or measure native UI cancellation. Cache state is unknown. No raw stdout/stderr is persisted.",
        "samples": [],
    }
    result_path = output / "summary.json"
    try:
        for number in range(1, args.samples + 1):
            record = sample(command, number, args.cancel_after_ms, args.timeout_seconds)
            report["samples"].append(record)
            result_path.write_text(json.dumps(report, indent=2) + "\n")
            print(f"Cancellation {number}/{args.samples}: {'pass' if record['passed'] else 'FAIL'}, upper bound {record['wall_minus_delay_upper_bound_ms']:.3f} ms", file=sys.stderr, flush=True)
        report["cli_binary_sha256_after"] = sha256(cli)
        report["fixture_marker_unchanged"] = sha256(marker_path) == report["fixture_marker_sha256"]
        report["binary_unchanged"] = report["cli_binary_sha256_after"] == report["cli_binary_sha256"]
        valid = [item["wall_minus_delay_upper_bound_ms"] for item in report["samples"] if item["passed"]]
        report["summary"] = {
            "valid_samples": len(valid),
            "failed_samples": len(report["samples"]) - len(valid),
            "median_upper_bound_ms": statistics.median(valid) if valid else None,
            "p95_upper_bound_ms": helper.percentile95(valid),
            "maximum_upper_bound_ms": max(valid) if valid else None,
            "all_upper_bounds_within_target": len(valid) == args.samples and max(valid) <= args.target_ms,
        }
        report["passed"] = len(valid) == args.samples and report["binary_unchanged"] and report["fixture_marker_unchanged"]
        report["status"] = "complete"
        report["completed_at"] = utc_now()
    except BaseException as error:
        report.update(status="interrupted", passed=False, interruption=type(error).__name__)
        raise
    finally:
        result_path.write_text(json.dumps(report, indent=2) + "\n")
    print(str(result_path))
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except KeyboardInterrupt:
        print("Cancellation benchmark interrupted; completed samples were retained.", file=sys.stderr)
        raise SystemExit(130)
    except (OSError, ValueError, RuntimeError) as error:
        print(f"Cancellation benchmark stopped: {error}", file=sys.stderr)
        raise SystemExit(1)
