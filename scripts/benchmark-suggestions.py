#!/usr/bin/env python3
"""Compare two Suggestions scanners on the same marked disposable fixture."""

from __future__ import annotations

import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import platform
import statistics
import sys

import benchmark


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024**2), b""):
            digest.update(chunk)
    return digest.hexdigest()


def eligible_proofs(output: Path, root: Path) -> list[dict]:
    """Compare final eligible rows; provisional and diagnostic counts may differ."""
    eligible = {}
    with output.open("rb") as stream:
        while line := stream.readline(1024**2 + 1):
            if len(line) > 1024**2:
                raise ValueError("A scanner JSON line exceeded the bounded parser limit")
            item = json.loads(line)
            if not isinstance(item, dict):
                raise ValueError("Scanner output must contain JSON objects")
            candidates = item.get("candidates")
            if not isinstance(candidates, list):
                continue
            for candidate in candidates:
                if not isinstance(candidate, dict):
                    raise ValueError("A scanner candidate must be a JSON object")
                path = candidate["path"]
                if candidate.get("suggestion_eligible") is not True:
                    eligible.pop(path, None)
                    continue
                if candidate.get("provisional") is not False or candidate.get("blocked_reason") is not None:
                    raise ValueError("An eligible recommendation is provisional or blocked")
                identity = candidate.get("identity")
                if not isinstance(identity, dict) or not all(
                    key in identity for key in ("device", "inode", "mode", "size", "modified_ns", "changed_ns")
                ) or not candidate.get("fingerprint"):
                    raise ValueError("An eligible recommendation lacks identity or fingerprint evidence")
                relative = Path(path).relative_to(root)
                eligible[path] = {
                    "path": str(relative),
                    "identity": identity,
                    "fingerprint": candidate["fingerprint"],
                    "logical_bytes": candidate["logical_bytes"],
                    "allocated_bytes": candidate["allocated_bytes"],
                    "file_count": candidate["file_count"],
                    "kind": candidate["kind"],
                    "eligible_permanent": candidate["eligible_permanent"],
                }
    return sorted(eligible.values(), key=lambda candidate: candidate["path"])


def summarize(records: list[dict]) -> dict:
    result = {}
    for name in ("baseline", "candidate"):
        samples = [record for record in records if record["tool"] == name]
        warm = [record for record in samples if record["phase"] == "warm" and record.get("verified")]
        times = [record["wall_ms"] for record in warm]
        cpu = [record["user_cpu_seconds"] + record["system_cpu_seconds"] for record in warm]
        result[name] = {
            "verified_runs": sum(record.get("verified", False) for record in samples),
            "total_runs": len(samples),
            "first_run_wall_ms": samples[0]["wall_ms"] if samples else None,
            "warm_median_wall_ms": statistics.median(times) if times else None,
            "warm_p95_wall_ms": benchmark.percentile95(times),
            "warm_p95_first_eligible_ms": benchmark.percentile95([record["first_candidate_wall_ms"] for record in warm]),
            "warm_median_cpu_seconds": statistics.median(cpu) if cpu else None,
            "warm_p95_cpu_seconds": benchmark.percentile95(cpu),
            "maximum_peak_rss_bytes": max((record["peak_rss_bytes"] for record in samples if record.get("verified")), default=None),
        }
    return result


def markdown_report(report: dict) -> str:
    def number(value: float | None) -> str:
        return f"{value:.3f}" if value is not None else "—"

    lines = [
        "# Local Suggestions comparison", "",
        f"Recorded: {report['started_at']}", "",
        "Both binaries run recommendation discovery on the same audited fixture. "
        "Each round alternates execution order. Creation and the pre-run audit warm filesystem caches; "
        "first invocation is not a cold-cache measurement.", "",
        "| Variant | Verified runs | First (ms) | Warm p95 (ms) | First eligible p95 (ms) | CPU p95 (s) | Peak RSS (MiB) |",
        "| --- | ---: | ---: | ---: | ---: | ---: | ---: |",
    ]
    for name, item in report["summary"].items():
        rss = item["maximum_peak_rss_bytes"]
        lines.append(
            f"| {name} | {item['verified_runs']}/{item['total_runs']} | {number(item['first_run_wall_ms'])} | "
            f"{number(item['warm_p95_wall_ms'])} | {number(item['warm_p95_first_eligible_ms'])} | "
            f"{number(item['warm_p95_cpu_seconds'])} | {number(rss / 1024**2 if rss is not None else None)} |"
        )
    lines.extend(["", f"Fixture audit and identical recommendation proofs: {'PASS' if report['verified'] else 'FAIL / incomplete'}.", ""])
    if report.get("failure"):
        lines.extend([f"Failure: {report['failure']}", ""])
    lines.extend([
        "Eligible paths, identities, fingerprints, sizes and cleanup eligibility must match across every run. "
        "Traversal entry totals and diagnostic counts are not equivalence criteria: discovery may skip unrelated metadata.", "",
        "These are whole CLI process measurements. They do not measure native UI latency or idle CPU, "
        "or establish performance relative to tools performing exhaustive inventory.", "",
        "Exact binary and harness hashes, hardware, commands, CPU/RSS readings, audits and raw output filenames are in summary.json.", "",
    ])
    return "\n".join(lines)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("fixture", type=Path)
    parser.add_argument("--baseline-cli", type=Path, required=True)
    parser.add_argument("--candidate-cli", type=Path, required=True)
    parser.add_argument("--warm-runs", type=int, default=5)
    parser.add_argument("--timeout", type=float, default=300, help="Maximum seconds per invocation")
    parser.add_argument("--output", type=Path, help="New directory under benchmarks/local")
    args = parser.parse_args(argv)
    if platform.system() != "Darwin":
        parser.error("This harness requires Darwin /usr/bin/time -l CPU and RSS measurements")
    if not 1 <= args.warm_runs <= 100 or not 1 <= args.timeout <= 3600:
        parser.error("Use 1..100 warm runs and a 1..3600 second timeout")
    requested = args.fixture.expanduser()
    if requested.is_symlink():
        parser.error("Fixture root must not be a symlink")
    fixture = requested.resolve(strict=True)
    marker_path = fixture / benchmark.MARKER
    if marker_path.is_symlink() or not marker_path.is_file():
        parser.error("A regular fixture marker is required")
    marker = json.loads(marker_path.read_text())
    if marker.get("magic") != benchmark.MAGIC or marker.get("status") != "complete" or marker.get("baseline_relative_path") != "baseline":
        parser.error("A complete, marked Chippytea fixture is required")
    root = fixture / "baseline"
    if root.is_symlink() or not root.is_dir():
        parser.error("The marked baseline must be a real directory")
    binaries = {name: getattr(args, name + "_cli").expanduser().resolve(strict=True) for name in ("baseline", "candidate")}
    if any(not path.is_file() or not os.access(path, os.X_OK) for path in binaries.values()):
        parser.error("Both CLI paths must identify existing executable files")
    stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%S.%fZ")
    local = (benchmark.REPO / "benchmarks/local").resolve()
    output = (args.output or local / f"suggestions-{stamp}").expanduser().resolve()
    if local not in output.parents or output == fixture or fixture in output.parents:
        parser.error("Results must be in a new benchmarks/local directory outside the fixture")
    output.mkdir(parents=True, exist_ok=False)
    root_metadata = root.stat()
    root_identity = {"device": root_metadata.st_dev, "inode": root_metadata.st_ino}
    report = {
        "schema_version": 1,
        "started_at": datetime.now(timezone.utc).isoformat(),
        "hardware": benchmark.hardware(),
        "fixture": {"path": str(fixture), "baseline": str(root), "root_identity": root_identity, "marker": marker, "marker_sha256": sha256(marker_path)},
        "binaries": {name: {"path": str(path), "sha256": sha256(path)} for name, path in binaries.items()},
        "harness_sha256": sha256(Path(__file__)),
        "shared_helpers_sha256": sha256(Path(benchmark.__file__)),
        "method": {"mode": "Suggestions", "warm_runs": args.warm_runs, "order": "baseline/candidate in even rounds; candidate/baseline in odd rounds", "cache_state": "Unknown; creation and pre-run audit warm caches", "percentile": "nearest rank", "timed_scope": "whole child process", "first_eligible": "first eligible JSON batch observed on stdout"},
        "runs": [], "verified": False,
    }
    summary_path = output / "summary.json"
    try:
        report["audit_before"] = benchmark.verify_fixture(root, marker["baseline"])
        report["audit_before"]["performed"] = "before all timed runs; warms filesystem caches"
        if not report["audit_before"]["matches_marker"]:
            raise ValueError("Fixture does not match its marker before timing")
        reference = None
        for round_number in range(args.warm_runs + 1):
            order = ("baseline", "candidate") if round_number % 2 == 0 else ("candidate", "baseline")
            phase = "first" if round_number == 0 else "warm"
            for name in order:
                expected_hash = report["binaries"][name]["sha256"]
                if sha256(binaries[name]) != expected_hash:
                    raise ValueError(f"{name} CLI changed before invocation")
                print(f"Round {round_number}: {name} ({phase})", file=sys.stderr, flush=True)
                record = benchmark.run_sample(name, phase, round_number or 1, [str(binaries[name]), "scan", str(root)], output, args.timeout)
                report["runs"].append(record)
                record.update(round=round_number, binary_sha256=expected_hash, verified=False)
                stats = record.get("engine_stats") or {}
                if record["exit_code"] != 0 or record["timed_out"] or stats.get("complete") is not True or stats.get("cancelled") or stats.get("errors") != 0:
                    raise ValueError(f"{name} scan did not complete successfully")
                if sha256(binaries[name]) != expected_hash:
                    raise ValueError(f"{name} CLI changed during invocation")
                if record["json_line_exceeded_1_mib"] or any(record.get(key) is None for key in ("user_cpu_seconds", "system_cpu_seconds", "peak_rss_bytes", "first_candidate_wall_ms")):
                    raise ValueError(f"{name} lacks bounded output, timing, memory or first-finding evidence")
                proof = eligible_proofs(output / record["stdout_artifact"], root)
                if not proof:
                    raise ValueError("A Suggestions comparison fixture must produce at least one eligible recommendation")
                expected_paths = marker.get("expected_eligible_relative_paths")
                if expected_paths is not None:
                    if (not isinstance(expected_paths, list)
                            or not expected_paths
                            or not all(isinstance(path, str) for path in expected_paths)
                            or len(set(expected_paths)) != len(expected_paths)):
                        raise ValueError("The fixture's expected eligible paths must be distinct strings")
                    if [item["path"] for item in proof] != sorted(expected_paths):
                        raise ValueError(f"{name} did not return the fixture's exact expected eligible paths")
                record["eligible_candidates"] = proof
                record["eligible_sha256"] = hashlib.sha256(json.dumps(proof, sort_keys=True, separators=(",", ":")).encode()).hexdigest()
                if reference is None:
                    reference = proof
                if proof != reference:
                    raise ValueError(f"{name} eligible recommendation proof differs from the first baseline run")
                record["verified"] = True
                summary_path.write_text(json.dumps(report, indent=2) + "\n")
        report["audit_after"] = benchmark.verify_fixture(root, marker["baseline"])
        current_root = root.stat()
        if not report["audit_after"]["matches_marker"] or sha256(marker_path) != report["fixture"]["marker_sha256"] or {"device": current_root.st_dev, "inode": current_root.st_ino} != root_identity:
            raise ValueError("Fixture or marker changed during comparison")
        report["verified"] = True
    except (OSError, ValueError, KeyboardInterrupt) as error:
        report["failure"] = f"{type(error).__name__}: {error}"
        print(report["failure"], file=sys.stderr)
    finally:
        report["completed_at"] = datetime.now(timezone.utc).isoformat()
        report["summary"] = summarize(report["runs"])
        summary_path.write_text(json.dumps(report, indent=2) + "\n")
        (output / "REPORT.md").write_text(markdown_report(report))
    print(summary_path)
    return 0 if report["verified"] else 1


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, ValueError, KeyboardInterrupt) as error:
        print(f"Comparison stopped: {error}", file=sys.stderr)
        raise SystemExit(1)
