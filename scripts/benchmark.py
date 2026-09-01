#!/usr/bin/env python3
"""Measure CLI discovery of a marked disposable fixture without changing its files."""

from __future__ import annotations

import argparse
from datetime import datetime, timezone
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import re
import selectors
import shutil
import signal
import stat
import statistics
import subprocess
import sys
import time


REPO = Path(__file__).resolve().parents[1]
MARKER = ".chippytea-benchmark-fixture.json"
MAGIC = "chippytea-disposable-benchmark-v1"


def command_text(command: list[str]) -> str:
    try:
        result = subprocess.run(command, capture_output=True, text=True, timeout=10, check=False)
        return result.stdout.strip()[:2000] if result.returncode == 0 else "unavailable"
    except (OSError, subprocess.TimeoutExpired):
        return "unavailable"


def hardware() -> dict:
    # Do not use system_profiler: its output can include hardware serials and UUIDs.
    return {
        "model": command_text(["/usr/sbin/sysctl", "-n", "hw.model"]),
        "chip": command_text(["/usr/sbin/sysctl", "-n", "machdep.cpu.brand_string"]),
        "physical_cores": command_text(["/usr/sbin/sysctl", "-n", "hw.physicalcpu"]),
        "logical_cores": command_text(["/usr/sbin/sysctl", "-n", "hw.logicalcpu"]),
        "memory_bytes": command_text(["/usr/sbin/sysctl", "-n", "hw.memsize"]),
        "architecture": platform.machine(),
        "macos_version": command_text(["/usr/bin/sw_vers", "-productVersion"]),
        "macos_build": command_text(["/usr/bin/sw_vers", "-buildVersion"]),
    }


def percentile95(values: list[float]) -> float | None:
    return sorted(values)[math.ceil(len(values) * 0.95) - 1] if values else None


def stop_process(process: subprocess.Popen) -> None:
    if process.poll() is not None:
        return
    try:
        os.killpg(process.pid, signal.SIGTERM)
        process.wait(timeout=2)
    except subprocess.TimeoutExpired:
        os.killpg(process.pid, signal.SIGKILL)
        process.wait(timeout=2)
    except ProcessLookupError:
        pass


def tail_text(path: Path, maximum: int = 256 * 1024) -> str:
    with path.open("rb") as stream:
        stream.seek(max(0, path.stat().st_size - maximum))
        return stream.read().decode("utf-8", errors="replace")


def run_sample(name: str, phase: str, number: int, command: list[str], output: Path, timeout: float) -> dict:
    prefix = f"{name}-{phase}-{number:02d}"
    stdout_path = output / f"{prefix}.stdout"
    stderr_path = output / f"{prefix}.stderr"
    env = {key: value for key, value in os.environ.items() if not key.startswith(("DUA_", "DUST_"))}
    env.update(LC_ALL="C", LANG="C")
    env.pop("RAYON_NUM_THREADS", None)
    timed_command = ["/usr/bin/time", "-l", *command]
    started = time.monotonic()
    process = subprocess.Popen(timed_command, stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env, start_new_session=True)
    first_stdout_ms = None
    first_candidate_ms = None
    engine_stats = None
    parse_buffer = b""
    parse_overflow = False
    discard_line = False
    timed_out = False

    def observe_line(line: bytes) -> None:
        nonlocal first_candidate_ms, engine_stats
        try:
            item = json.loads(line)
        except (ValueError, UnicodeError):
            return
        if not isinstance(item, dict):
            return
        if first_candidate_ms is None and isinstance(item.get("candidates"), list) and any(
            candidate.get("suggestion_eligible") is True for candidate in item["candidates"]
        ):
            first_candidate_ms = (time.monotonic() - started) * 1000
        stats = item.get("stats", item)
        if isinstance(stats, dict) and "entries" in stats and "complete" in stats:
            engine_stats = stats

    try:
        with stdout_path.open("xb") as out, stderr_path.open("xb") as err, selectors.DefaultSelector() as selector:
            selector.register(process.stdout, selectors.EVENT_READ, out)
            selector.register(process.stderr, selectors.EVENT_READ, err)
            while selector.get_map():
                if time.monotonic() - started > timeout:
                    timed_out = True
                    stop_process(process)
                for key, _ in selector.select(timeout=0.1):
                    chunk = os.read(key.fileobj.fileno(), 65_536)
                    if not chunk:
                        selector.unregister(key.fileobj)
                        key.fileobj.close()
                        continue
                    key.data.write(chunk)
                    if key.data is out:
                        if first_stdout_ms is None:
                            first_stdout_ms = (time.monotonic() - started) * 1000
                        if discard_line:
                            newline = chunk.find(b"\n")
                            if newline == -1:
                                continue
                            chunk, discard_line = chunk[newline + 1 :], False
                        parse_buffer += chunk
                        while b"\n" in parse_buffer:
                            line, parse_buffer = parse_buffer.split(b"\n", 1)
                            observe_line(line)
                        if len(parse_buffer) > 1024**2:
                            parse_buffer, parse_overflow, discard_line = b"", True, True
            if parse_buffer and not discard_line:
                observe_line(parse_buffer)
            code = process.wait(timeout=2)
    finally:
        stop_process(process)
    wall_ms = (time.monotonic() - started) * 1000
    stderr = tail_text(stderr_path)
    cpu = re.search(r"([\d.]+)\s+real\s+([\d.]+)\s+user\s+([\d.]+)\s+sys", stderr)
    rss = re.search(r"(\d+)\s+maximum resident set size", stderr)
    record = {
        "tool": name,
        "phase": phase,
        "sample": number,
        "command": command,
        "exit_code": code,
        "timed_out": timed_out,
        "wall_ms": round(wall_ms, 3),
        "time_real_seconds": float(cpu[1]) if cpu else None,
        "user_cpu_seconds": float(cpu[2]) if cpu else None,
        "system_cpu_seconds": float(cpu[3]) if cpu else None,
        "peak_rss_bytes": int(rss[1]) if rss else None,
        "rss_source": "Darwin /usr/bin/time -l maximum resident set size, bytes",
        "first_stdout_ms": round(first_stdout_ms, 3) if first_stdout_ms is not None else None,
        "first_candidate_wall_ms": round(first_candidate_ms, 3) if first_candidate_ms is not None else None,
        "engine_stats": engine_stats,
        "json_line_exceeded_1_mib": parse_overflow,
        "stdout_artifact": stdout_path.name,
        "stderr_artifact": stderr_path.name,
    }
    if name == "du":
        match = re.match(r"\s*(\d+)\s", tail_text(stdout_path))
        record["reported_allocated_bytes"] = int(match[1]) * 1024 if match else None
    elif name == "dua":
        # aggregate lists individual children before its total. The first row
        # is not the root's allocation when a directory has multiple children.
        match = re.search(r"^\s*(\d+)\s+b\s+total\s*$", tail_text(stdout_path), re.MULTILINE)
        record["reported_allocated_bytes"] = int(match[1]) if match else None
        entries = re.search(r"\bentries_traversed:\s*(\d+)\b", stderr)
        record["reported_entries_excluding_root"] = int(entries[1]) if entries else None
    return record


def verify_fixture(root: Path, expected: dict) -> dict:
    """Independent post-timing coverage audit; never follow symlinks or read file data."""
    pending = [root]
    files = directories = links = special = logical = allocated = 0
    errors = []
    while pending:
        directory = pending.pop()
        try:
            metadata = directory.lstat()
            if not stat.S_ISDIR(metadata.st_mode):
                errors.append("A directory changed type during coverage verification")
                continue
            directories += 1
            allocated += metadata.st_blocks * 512
            with os.scandir(directory) as entries:
                for entry in entries:
                    metadata = entry.stat(follow_symlinks=False)
                    if stat.S_ISDIR(metadata.st_mode):
                        pending.append(Path(entry.path))
                    elif stat.S_ISREG(metadata.st_mode):
                        files += 1
                        logical += metadata.st_size
                        allocated += metadata.st_blocks * 512
                        if metadata.st_nlink != 1:
                            errors.append("Unexpected hard-linked file in independent-file baseline")
                    elif stat.S_ISLNK(metadata.st_mode):
                        links += 1
                    else:
                        special += 1
        except OSError as error:
            errors.append(f"{type(error).__name__}: {error.errno}")
        if len(errors) > 100:
            errors.append("Coverage audit stopped after more than 100 errors")
            break
    measured = {
        "files": files,
        "directories_including_root": directories,
        "entries_including_root": files + directories + links + special,
        "entries_excluding_root": files + directories + links + special - 1,
        "logical_regular_file_bytes": logical,
        "allocated_bytes_including_directories": allocated,
        "symlinks": links,
        "special_files": special,
    }
    checks = {key: measured[key] == expected[key] for key in measured if key in expected}
    return {"performed": "after all timed runs", "measured": measured, "matches_marker": all(checks.values()) and not errors, "checks": checks, "errors": errors}


def summarize(records: list[dict], expected: dict) -> dict:
    summaries = {}
    for name in dict.fromkeys(item["tool"] for item in records):
        samples = [item for item in records if item["tool"] == name]
        good = [item for item in samples if item["exit_code"] == 0 and not item["timed_out"]]
        warm = [item for item in good if item["phase"] == "warm"]
        findings = [item["first_candidate_wall_ms"] for item in warm if item["first_candidate_wall_ms"] is not None]
        engine = [item["engine_stats"] for item in samples if item["engine_stats"] is not None]
        coverage = None
        if name in ("chippytea-traverse", "chippytea-discovery", "chippytea-index"):
            coverage = len(engine) == len(samples) and all(
                item.get("complete") is True
                and not item.get("cancelled")
                and item.get("errors") == 0
                and item.get("files") == expected["files"]
                and item.get("entries") in (expected["entries_including_root"], expected["entries_excluding_root"])
                for item in engine
            )
        summaries[name] = {
            "successful_samples": len(good),
            "total_samples": len(samples),
            "first_run_wall_ms": samples[0]["wall_ms"] if samples else None,
            "warm_samples": len(warm),
            "warm_median_wall_ms": statistics.median(item["wall_ms"] for item in warm) if warm else None,
            "warm_p95_wall_ms": percentile95([item["wall_ms"] for item in warm]),
            "maximum_peak_rss_bytes": max((item["peak_rss_bytes"] for item in good if item["peak_rss_bytes"] is not None), default=None),
            "warm_p95_first_candidate_wall_ms": percentile95(findings),
            "warm_samples_with_findings": len(findings),
            "complete_engine_coverage_matches_fixture": coverage,
            "coverage_note": "Engine counts compared with exact fixture counts" if coverage is not None else "Fixed, audited baseline root; this tool does not report comparable entry counts here",
        }
    return summaries


def markdown_report(report: dict) -> str:
    lines = [
        "# Local traversal benchmark",
        "",
        f"Recorded: {report['started_at']}",
        "",
        "Cache state is unknown. First run means the first timed invocation of each tool; "
        "earlier tools and fixture creation may already have warmed caches. No cold-cache claim is made.",
        "",
        "| Tool | Successful runs | First (ms) | Warm p95 (ms) | Peak RSS (MiB) |",
        "| --- | ---: | ---: | ---: | ---: |",
    ]
    for name, item in report.get("summary", {}).items():
        first = f"{item['first_run_wall_ms']:.3f}" if item["first_run_wall_ms"] is not None else "—"
        warm = f"{item['warm_p95_wall_ms']:.3f}" if item["warm_p95_wall_ms"] is not None else "—"
        rss = f"{item['maximum_peak_rss_bytes'] / 1024**2:.2f}" if item["maximum_peak_rss_bytes"] is not None else "—"
        lines.append(f"| {name} | {item['successful_samples']}/{item['total_samples']} | {first} | {warm} | {rss} |")
    lines.extend(["", "Unavailable optional tools: " + (", ".join(report["missing_optional_tools"]) or "none"), ""])
    lines.append("Post-run fixture coverage: " + ("matches marker" if report.get("coverage", {}).get("matches_marker") else "not verified / mismatch"))
    lines.extend([
        "",
        "These are CLI process measurements. Native window latency, frame responsiveness, "
        "idle CPU, and cancellation latency require the app's separate native/safety harness; "
        "they are not inferred from traversal timings.",
        "",
        "Exact hardware, commands, versions, CPU times, counts, first-findings timing, "
        "exit statuses and raw output paths are in summary.json.",
        "",
    ])
    return "\n".join(lines)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("fixture", type=Path, help="Fixture directory containing the chippytea marker")
    parser.add_argument("--cli", type=Path, default=REPO / "target/release/chippytea-cli")
    parser.add_argument("--warm-runs", type=int, default=5, help="Warm invocations after one first run (default: 5)")
    parser.add_argument("--threads", type=int, default=4, help="dua/dust worker count")
    parser.add_argument("--timeout", type=float, default=300, help="Maximum seconds per child process")
    parser.add_argument("--skip-discovery", action="store_true", help="Run equivalent traversal only")
    parser.add_argument("--include-index", action="store_true", help="Also measure the persistent engine with a fresh disposable SQLite index")
    parser.add_argument("--output", type=Path, help="New results directory (default: benchmarks/local/<timestamp>)")
    args = parser.parse_args(argv)
    if platform.system() != "Darwin":
        parser.error("This harness requires macOS /usr/bin/time -l; RSS units are Darwin bytes")
    if not 1 <= args.warm_runs <= 100 or not 1 <= args.threads <= 64 or not 1 <= args.timeout <= 3600:
        parser.error("Use 1..100 warm runs, 1..64 threads and a 1..3600 second timeout")
    if args.fixture.is_symlink():
        parser.error("Fixture root must not be a symlink")
    fixture = args.fixture.expanduser().resolve(strict=True)
    marker = json.loads((fixture / MARKER).read_text())
    if marker.get("magic") != MAGIC or marker.get("status") != "complete" or marker.get("baseline_relative_path") != "baseline":
        parser.error("A complete, marked chippytea fixture is required")
    baseline = fixture / "baseline"
    if baseline.is_symlink() or not baseline.is_dir():
        parser.error("Baseline must be a real directory")
    cli = args.cli.expanduser().resolve()
    if not cli.is_file() or not os.access(cli, os.X_OK):
        parser.error(f"Build the release CLI first: {cli}")
    stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%S.%fZ")
    output = (args.output or REPO / "benchmarks/local" / stamp).expanduser().resolve()
    if output == fixture or fixture in output.parents:
        parser.error("Results must be outside the fixture to keep timed coverage unchanged")
    output.mkdir(parents=True, exist_ok=False)
    hardware_info = hardware()
    available = {name: shutil.which(name) for name in ("dua", "dust")}
    commands = {
        "chippytea-traverse": [str(cli), "traverse", str(baseline)],
        "du": ["/usr/bin/du", "-s", "-k", "-x", str(baseline)],
    }
    if available["dua"]:
        commands["dua"] = [available["dua"], "aggregate", "--threads", str(args.threads), "--stay-on-filesystem", "--format", "bytes", "--no-sort", "--stats", str(baseline)]
    if available["dust"]:
        commands["dust"] = [available["dust"], "--threads", str(args.threads), "--config", "/dev/null", "--limit-filesystem", "--no-progress", "--no-colors", "--no-percent-bars", "--depth", "1", "--output-format", "b", str(baseline)]
    if not args.skip_discovery:
        commands["chippytea-discovery"] = [str(cli), "scan", str(baseline), "--metadata-coverage"]
    if args.include_index:
        commands["chippytea-index"] = [str(cli), "index", str(baseline), "--metadata-coverage"]
    versions = {name: command_text([path, "--version"]) for name, path in available.items() if path}
    versions["du"] = f"Apple /usr/bin/du shipped with macOS {hardware_info['macos_version']} ({hardware_info['macos_build']})"
    package_version = re.search(r'^version\s*=\s*"([^"]+)"', (REPO / "Cargo.toml").read_text(), re.MULTILINE)
    versions["chippytea-cli"] = {
        "source_package_version": package_version[1] if package_version else "unavailable",
        "note": "Source package version; the measured binary is identified independently by SHA-256 below",
    }
    report = {
        "schema_version": 1,
        "started_at": datetime.now(timezone.utc).isoformat(),
        "hardware": hardware_info,
        "fixture": {"path": str(fixture), "baseline": str(baseline), "marker": marker},
        "cache_state": "Unknown. First run is first timed invocation per tool; creation and earlier tools warm caches.",
        "method": {"warm_runs": args.warm_runs, "tool_threads": args.threads, "percentile": "nearest rank", "timed_scope": "whole child process", "coverage_audit": "after all timed runs", "symlinks": "not followed", "filesystem_boundary": "root filesystem only"},
        "versions": versions,
        "cli_binary_sha256": hashlib.sha256(cli.read_bytes()).hexdigest(),
        "git_head": command_text(["git", "-C", str(REPO), "rev-parse", "HEAD"]),
        "missing_optional_tools": [name for name, path in available.items() if not path],
        "runs": [],
        "native_metrics": {name: "not measured by this CLI harness" for name in ("warm_window_p95_ms", "idle_cpu_percent", "cancellation_latency_ms", "ui_responsiveness")},
    }
    summary_path = output / "summary.json"
    try:
        for name, command in commands.items():
            for index in range(args.warm_runs + 1):
                phase = "first" if index == 0 else "warm"
                print(f"{name}: {phase} run {index or 1}", file=sys.stderr, flush=True)
                record = run_sample(name, phase, index or 1, command, output, args.timeout)
                report["runs"].append(record)
                summary_path.write_text(json.dumps(report, indent=2) + "\n")
                if record["exit_code"] != 0 or record["timed_out"]:
                    print(f"{name} failed; further repetitions skipped. See {record['stderr_artifact']}", file=sys.stderr)
                    break
        report["coverage"] = verify_fixture(baseline, marker["baseline"])
        report["summary"] = summarize(report["runs"], marker["baseline"])
        report["completed_at"] = datetime.now(timezone.utc).isoformat()
    finally:
        summary_path.write_text(json.dumps(report, indent=2) + "\n")
    (output / "REPORT.md").write_text(markdown_report(report))
    print(str(summary_path))
    failures = any(item["exit_code"] != 0 or item["timed_out"] for item in report["runs"])
    incomplete = any(
        item["complete_engine_coverage_matches_fixture"] is not True
        for name, item in report["summary"].items()
        if name.startswith("chippytea-")
    )
    return 1 if failures or incomplete or not report.get("coverage", {}).get("matches_marker") else 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, ValueError, KeyboardInterrupt) as error:
        print(f"Benchmark stopped: {error}", file=sys.stderr)
        raise SystemExit(1)
