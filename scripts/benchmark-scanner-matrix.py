#!/usr/bin/env python3
"""Pair Suggestions scanners on newly generated, disposable source/log/download fixtures.

No existing folder can be scanned, overwritten or deleted by this harness. Fixture
creation, binary hashing and full metadata audits are outside all timed samples.
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
import shutil
import stat
import statistics
import subprocess
import sys
import time

import benchmark


MAGIC = "chippytea-disposable-scanner-matrix-v1"
MIB = 1024**2
RESERVE_BYTES = 3 * 1024**3
MAX_FIXTURE_PAYLOAD_BYTES = 64 * MIB
DEPTH = 64
OLD_AGE_SECONDS = 400 * 24 * 60 * 60
CASES = (
    ("source-wide", "projects", "Wide directory of empty source files", None),
    ("source-deep", "folder", "Empty source files spread across 64 directory levels", None),
    ("downloads", "downloads", "Recent sparse installers plus one old allocated installer", "installer"),
    ("home-logs", "home", "Old sparse logs plus one old allocated log", "log"),
)


def load_suggestions_helpers():
    specification = importlib.util.spec_from_file_location(
        "chippytea_suggestions_helpers", Path(__file__).with_name("benchmark-suggestions.py")
    )
    if specification is None or specification.loader is None:
        raise ValueError("Cannot load the adjacent Suggestions benchmark helpers")
    module = importlib.util.module_from_spec(specification)
    specification.loader.exec_module(module)
    return module


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(MIB), b""):
            digest.update(chunk)
    return digest.hexdigest()


def json_digest(value) -> str:
    return hashlib.sha256(json.dumps(value, sort_keys=True, separators=(",", ":")).encode()).hexdigest()


def write_json(path: Path, value, *, exclusive: bool = False) -> None:
    with path.open("x" if exclusive else "w", encoding="utf-8") as stream:
        json.dump(value, stream, sort_keys=True, separators=(",", ":"))
        stream.write("\n")


def space_guard(path: Path, additional_bytes: int = 0) -> dict:
    available = shutil.disk_usage(path).free
    required = RESERVE_BYTES + additional_bytes
    if available < required:
        raise ValueError(f"Space guard: {available:,} bytes free; at least {required:,} required")
    return {"free_bytes": available, "reserve_bytes": RESERVE_BYTES, "additional_estimated_bytes": additional_bytes}


def metadata(path: Path, relative: str) -> dict:
    info = path.lstat()
    if stat.S_ISDIR(info.st_mode):
        kind = "directory"
    elif stat.S_ISREG(info.st_mode) and info.st_nlink == 1:
        kind = "file"
    else:
        raise ValueError(f"Fixture contains a symlink, special file or hard link: {path}")
    return {
        "path": relative, "type": kind,
        "device": info.st_dev, "inode": info.st_ino, "mode": info.st_mode,
        "links": info.st_nlink, "uid": info.st_uid, "gid": info.st_gid,
        "size": info.st_size, "allocated_bytes": info.st_blocks * 512,
        "modified_ns": info.st_mtime_ns, "changed_ns": info.st_ctime_ns,
        "birthtime_ns": getattr(info, "st_birthtime_ns", int(getattr(info, "st_birthtime", 0) * 1_000_000_000)),
        "flags": getattr(info, "st_flags", 0),
    }


def manifest(root: Path) -> list[dict]:
    """Audit every entry without opening data files; access times are intentionally excluded."""
    result = []
    pending = [root]
    while pending:
        path = pending.pop()
        entry = metadata(path, str(path.relative_to(root)))
        result.append(entry)
        if entry["type"] == "directory":
            # No fixture contains links. Recheck identity after enumeration to
            # reject a replaced directory rather than accepting a partial audit.
            with os.scandir(path) as children:
                pending.extend(Path(child.path) for child in children)
            if metadata(path, entry["path"]) != entry:
                raise ValueError(f"Fixture directory changed during its audit: {path}")
    return sorted(result, key=lambda entry: entry["path"])


def audit_record(entries: list[dict], artifact: str) -> dict:
    files = [entry for entry in entries if entry["type"] == "file"]
    return {
        "manifest_artifact": artifact, "canonical_manifest_sha256": json_digest(entries),
        "entries_including_root": len(entries), "files": len(files),
        "directories_including_root": len(entries) - len(files),
        "logical_regular_file_bytes": sum(entry["size"] for entry in files),
        "allocated_regular_file_bytes": sum(entry["allocated_bytes"] for entry in files),
        "allocated_bytes_including_directories": sum(entry["allocated_bytes"] for entry in entries),
    }


def sparse_file(path: Path, logical_bytes: int, old_ns: int | None = None) -> None:
    with path.open("xb") as stream:
        stream.truncate(logical_bytes)
    if old_ns is not None:
        os.utime(path, ns=(old_ns, old_ns), follow_symlinks=False)
    if path.lstat().st_blocks != 0:
        raise ValueError(f"Sparse fixture file unexpectedly allocated disk blocks: {path}")


def allocated_file(path: Path, mebibytes: int, old_ns: int) -> None:
    # Actual writes, not truncate/fallocate: this row must own enough locally
    # allocated space to be an independently known eligible recommendation.
    block = hashlib.shake_256(b"chippytea-scanner-matrix-payload-v1").digest(MIB)
    with path.open("xb") as stream:
        for _ in range(mebibytes):
            stream.write(block)
        stream.flush()
        os.fsync(stream.fileno())
    os.utime(path, ns=(old_ns, old_ns), follow_symlinks=False)
    if path.lstat().st_blocks * 512 < mebibytes * MIB:
        raise ValueError(f"Positive fixture payload was not fully allocated: {path}")


def generate_fixtures(output: Path, count: int, report: dict) -> dict[str, Path]:
    fixtures = output / "fixtures"
    fixtures.mkdir()
    generated_ns = time.time_ns()
    old_ns = generated_ns - OLD_AGE_SECONDS * 1_000_000_000
    roots = {}
    for name, kind, description, expected_kind in CASES:
        space_guard(output)
        root = fixtures / name
        root.mkdir()
        roots[name] = root
        item = {"path": str(root), "kind": kind, "description": description, "expected_eligible_paths": []}
        report["fixtures"][name] = item
        # The in-progress marker identifies even a retained, interrupted fixture.
        marker = root / benchmark.MARKER
        marker_data = {
            "magic": MAGIC, "status": "incomplete", "case": name,
            "root_kind": kind, "workload_files": count, "generated_ns": generated_ns,
            "old_file_modified_ns": old_ns, "disposable": True,
        }
        write_json(marker, marker_data, exclusive=True)
        if expected_kind is None:
            directories = [root]
            if name == "source-deep":
                for level in range(DEPTH):
                    child = directories[-1] / f"d{level:02d}"
                    child.mkdir()
                    directories.append(child)
                directories = directories[1:]
            extensions = ("rs", "ts", "swift", "py", "md", "json", "css", "txt")
            for number in range(count):
                directory = directories[number % len(directories)]
                with (directory / f"source-{number:08d}.{extensions[number % len(extensions)]}").open("xb"):
                    pass
        else:
            directory = root if name == "downloads" else root / "Library" / "Logs"
            if directory != root:
                directory.mkdir(parents=True)
            extension, size = ("dmg", 21) if name == "downloads" else ("log", 11)
            for number in range(count):
                sparse_file(directory / f"sparse-{number:08d}.{extension}", size * MIB, None if name == "downloads" else old_ns)
            positive = directory / f"old-allocated.{extension}"
            allocated_file(positive, size, old_ns)
            item["expected_eligible_paths"] = [str(positive.relative_to(root))]
            item["expected_kind"] = expected_kind
        marker_data["status"] = "complete"
        marker_data["expected_eligible_paths"] = item["expected_eligible_paths"]
        write_json(marker, marker_data)
        item["marker"] = marker_data
        print(f"Created {name}: {count:,} workload files", file=sys.stderr, flush=True)
    return roots


def verify_eligible(proofs: list[dict], case: dict, entries: list[dict]) -> None:
    if [proof["path"] for proof in proofs] != case["expected_eligible_paths"]:
        raise ValueError("Eligible recommendations do not match the exact independently expected fixture paths")
    by_path = {entry["path"]: entry for entry in entries}
    for proof in proofs:
        entry = by_path[proof["path"]]
        expected = {
            "kind": case["expected_kind"], "eligible_permanent": False,
            "logical_bytes": entry["size"], "allocated_bytes": entry["allocated_bytes"], "file_count": 1,
            "identity": {key: entry[key] for key in ("device", "inode", "mode", "size", "modified_ns", "changed_ns")},
        }
        if any(proof[key] != value for key, value in expected.items()):
            raise ValueError("An eligible recommendation does not match its independent metadata or Trash-only policy")


def require_measurements(record: dict) -> dict:
    if record["exit_code"] != 0 or record["timed_out"] or record["json_line_exceeded_1_mib"]:
        raise ValueError("Scanner exited unsuccessfully, timed out, or exceeded the bounded JSON parser limit")
    if any(record.get(key) is None for key in ("time_real_seconds", "user_cpu_seconds", "system_cpu_seconds", "peak_rss_bytes")):
        raise ValueError("Darwin time did not provide complete wall, CPU and RSS measurements")
    stats = record.get("engine_stats") or {}
    if type(stats.get("errors")) is not int or stats["errors"] != 0:
        raise ValueError("Scanner did not report zero errors")
    return stats


def summarize(records: list[dict]) -> dict:
    result = {}
    for case, _, _, _ in CASES:
        variants = {}
        for name in ("baseline", "candidate"):
            samples = [r for r in records if r["case"] == case and r["tool"] == name]
            good = [r for r in samples if r.get("verified")]
            warm = [r for r in good if r["phase"] == "warm"]
            first = [r for r in good if r["phase"] == "first"]
            wall = [r["wall_ms"] for r in warm]
            cpu = [r["user_cpu_seconds"] + r["system_cpu_seconds"] for r in warm]
            eligible = [r["first_candidate_wall_ms"] for r in warm if r["first_candidate_wall_ms"] is not None]
            variants[name] = {
                "verified_runs": len(good), "total_runs": len(samples), "warm_runs": len(warm),
                "first_run_wall_ms": first[0]["wall_ms"] if first else None,
                "warm_median_wall_ms": statistics.median(wall) if wall else None,
                "warm_p95_wall_ms": benchmark.percentile95(wall),
                "warm_median_cpu_seconds": statistics.median(cpu) if cpu else None,
                "warm_p95_cpu_seconds": benchmark.percentile95(cpu),
                "warm_p95_first_eligible_ms": benchmark.percentile95(eligible),
                "warm_samples_with_eligible_findings": len(eligible),
                "maximum_peak_rss_bytes": max((r["peak_rss_bytes"] for r in good), default=None),
            }
        ratios = []
        for baseline in records:
            if baseline["case"] != case or baseline["tool"] != "baseline" or baseline["phase"] != "warm" or not baseline.get("verified"):
                continue
            candidate = next((r for r in records if r["case"] == case and r["tool"] == "candidate" and r["round"] == baseline["round"] and r.get("verified")), None)
            if candidate is not None:
                ratios.append(candidate["wall_ms"] / baseline["wall_ms"])
        result[case] = {"variants": variants, "warm_paired_candidate_over_baseline_wall_ratios": ratios,
                        "median_paired_wall_ratio": statistics.median(ratios) if ratios else None}
    return result


def markdown_report(report: dict) -> str:
    def number(value) -> str:
        return f"{value:.3f}" if value is not None else "—"

    lines = [
        "# Local Suggestions scanner matrix", "", f"Recorded: {report['started_at']}", "",
        "First invocations are separate from warm measurements. Fixture generation and metadata audits warm filesystem caches; these are not cold-cache measurements.", "",
        "| Fixture | Variant | Verified | First ms | Warm median ms | Warm p95 ms | Eligible p95 ms | CPU p95 s | Peak RSS MiB |",
        "| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |",
    ]
    for case, summary in report["summary"].items():
        for variant, item in summary["variants"].items():
            rss = item["maximum_peak_rss_bytes"]
            lines.append(f"| {case} | {variant} | {item['verified_runs']}/{item['total_runs']} | "
                         f"{number(item['first_run_wall_ms'])} | {number(item['warm_median_wall_ms'])} | "
                         f"{number(item['warm_p95_wall_ms'])} | {number(item['warm_p95_first_eligible_ms'])} | "
                         f"{number(item['warm_p95_cpu_seconds'])} | {number(rss / MIB if rss is not None else None)} |")
    lines.extend(["", f"Complete scans, exact eligible proofs, unchanged binaries and full fixture metadata: {'PASS' if report['verified'] else 'FAIL / incomplete'}.", ""])
    cancellation = report["cancellation"]
    if cancellation["requested"]:
        lines.extend([f"Cancellation: {'PASS' if cancellation.get('verified') else 'FAIL / not exercised'}.", "",
                      "A scan finishing before cancellation is reported as not exercised, never as successful cancellation. Wall minus requested delay is an upper bound including process startup and shutdown, not the exact cancel-signal latency.", ""])
    if report.get("failure"):
        lines.extend([f"Failure: {report['failure']}", ""])
    lines.extend([
        "Only source fixtures may return no eligible rows. Downloads and Home/Logs must each find exactly their allocated old file; sparse files must never become eligible. Eligible identities, fingerprints, kinds, sizes and permanent-cleanup flags must match between every scan of the same fixture.", "",
        "The matrix measures whole CLI processes on disposable local fixtures. It does not establish native UI performance, real user-disk coverage, or a ranking against other cleaners. Source-only cases must enumerate exactly the audited file, directory and entry counts, so an empty result cannot pass after a no-op scan. Other cases rely on exact positive findings because Home's selected scan routes can omit root-level entries.", "",
        "summary.json records hardware, exact commands, hashes, paired ratios, per-run metrics and audit results. Full metadata manifests and raw stdout/stderr are retained alongside it. No fixture is deleted.", "",
    ])
    return "\n".join(lines)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline-cli", type=Path, required=True)
    parser.add_argument("--candidate-cli", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True, help="New direct child directory of benchmarks/local")
    parser.add_argument("--warm-runs", type=int, default=5)
    parser.add_argument("--entries", type=int, default=10_000, help="Workload files per fixture; positive files and markers are additional")
    parser.add_argument("--timeout", type=float, default=300, help="Maximum seconds per CLI invocation")
    parser.add_argument("--cancel-after-ms", type=int, nargs="?", const=20, help="Opt into separate deep-fixture cancellation rounds (default delay when enabled: 20 ms)")
    args = parser.parse_args(argv)
    if platform.system() != "Darwin":
        parser.error("This harness requires Darwin /usr/bin/time -l measurements")
    if not 1 <= args.warm_runs <= 100 or not 1 <= args.entries <= 1_000_000:
        parser.error("Use 1..100 warm runs and 1..1,000,000 workload files per fixture")
    if not math.isfinite(args.timeout) or not 1 <= args.timeout <= 3600:
        parser.error("Use a finite 1..3600 second timeout")
    if args.cancel_after_ms is not None and not 1 <= args.cancel_after_ms <= min(10_000, args.timeout * 1000 - 500):
        parser.error("Cancellation delay must be 1..10,000 ms and at least 500 ms shorter than timeout")
    binaries = {name: getattr(args, name + "_cli").expanduser().resolve(strict=True) for name in ("baseline", "candidate")}
    if any(not path.is_file() or not os.access(path, os.X_OK) for path in binaries.values()):
        parser.error("Both CLI paths must be existing executable files")
    local = benchmark.REPO / "benchmarks" / "local"
    if local.is_symlink() or local.parent.is_symlink():
        parser.error("benchmarks/local and its parent must not be symlinks")
    requested = args.output.expanduser()
    if requested.is_symlink() or requested.exists():
        parser.error("Output must be a new directory; existing artifacts are never overwritten")
    output = requested.resolve()
    if output.parent != local:
        parser.error("Output must be a new direct child of benchmarks/local")
    if subprocess.run(["git", "check-ignore", "--quiet", "--", str(output)], cwd=benchmark.REPO, check=False).returncode != 0:
        parser.error("Output must be ignored by Git")
    local.mkdir(parents=True, exist_ok=True)
    # Budget 16 KiB of filesystem metadata plus two 1 KiB manifest rows per
    # entry. Actual fixture data is 32 MiB; sparse logical sizes consume none.
    estimated_entries = len(CASES) * args.entries + DEPTH + 16
    admission = space_guard(local, estimated_entries * (16 * 1024 + 2 * 1024) + 32 * MIB)
    helpers = load_suggestions_helpers()
    tracked = {"harness": Path(__file__).resolve(), "benchmark_helpers": Path(benchmark.__file__).resolve(),
               "suggestions_helpers": Path(helpers.__file__).resolve()}
    hashes = {name: sha256(path) for name, path in binaries.items()}
    output.mkdir(exist_ok=False)
    report = {
        "schema_version": 1, "started_at": datetime.now(timezone.utc).isoformat(),
        "hardware": benchmark.hardware(), "space_admission": admission,
        "binaries": {name: {"path": str(path), "sha256": hashes[name]} for name, path in binaries.items()},
        "harnesses": {name: {"path": str(path), "sha256": sha256(path)} for name, path in tracked.items()},
        "invocation": [sys.executable, str(Path(__file__).resolve()), *(argv if argv is not None else sys.argv[1:])],
        "method": {
            "mode": "Suggestions", "warm_runs": args.warm_runs, "workload_files_per_fixture": args.entries,
            "order": "For each fixture: baseline/candidate on even rounds; candidate/baseline on odd rounds",
            "cache_state": "Unknown; fixture creation and before-audits warm caches", "percentile": "nearest rank",
            "timed_scope": "Whole child process via Darwin /usr/bin/time -l",
            "first_eligible": "First eligible JSON batch observed on stdout; null for source-only cases",
            "metadata_audit": "Every path, type, identity, owner, link count, size, allocation, mtime, ctime, birthtime and flags; no file-content reads; atime excluded",
            "fixture_payload_limit_bytes": MAX_FIXTURE_PAYLOAD_BYTES,
        },
        "fixtures": {}, "runs": [], "verified": False,
        "cancellation": {"requested": args.cancel_after_ms is not None, "delay_ms": args.cancel_after_ms, "runs": []},
    }
    roots = {}
    manifests = {}
    reference = {}
    completed = False
    audit_ok = True
    summary_path = output / "summary.json"

    def run(case: str, name: str, phase: str, round_number: int, extra: list[str] | None = None) -> dict:
        space_guard(output)
        original_root = next(entry for entry in manifests[case] if entry["path"] == ".")
        if metadata(roots[case], ".") != original_root:
            raise ValueError(f"{case} fixture root changed before invocation")
        if sha256(binaries[name]) != hashes[name]:
            raise ValueError(f"{name} binary changed before invocation")
        command = [str(binaries[name]), "scan", str(roots[case]), "--kind", report["fixtures"][case]["kind"], *(extra or [])]
        print(f"{case}: round {round_number} {name} ({phase})", file=sys.stderr, flush=True)
        record = benchmark.run_sample(f"{case}-{name}", phase, round_number or 1, command, output, args.timeout)
        record.update(tool=name, case=case, round=round_number, binary_sha256=hashes[name], verified=False,
                      measured_command=["/usr/bin/time", "-l", *command])
        target = report["cancellation"]["runs"] if phase == "cancel" else report["runs"]
        target.append(record)
        if sha256(binaries[name]) != hashes[name]:
            raise ValueError(f"{name} binary changed during invocation")
        return record

    try:
        roots = generate_fixtures(output, args.entries, report)
        for case, root in roots.items():
            entries = manifest(root)
            manifests[case] = entries
            artifact = f"{case}.before-manifest.json"
            write_json(output / artifact, entries, exclusive=True)
            report["fixtures"][case]["audit_before"] = audit_record(entries, artifact)
        payload = sum(item["audit_before"]["allocated_regular_file_bytes"] for item in report["fixtures"].values())
        if payload > MAX_FIXTURE_PAYLOAD_BYTES:
            raise ValueError("Generated fixture payload exceeded the 64 MiB allocation limit")
        report["fixture_allocated_payload_bytes"] = payload
        for case, _, _, _ in CASES:
            for round_number in range(args.warm_runs + 1):
                order = ("baseline", "candidate") if round_number % 2 == 0 else ("candidate", "baseline")
                for name in order:
                    record = run(case, name, "first" if round_number == 0 else "warm", round_number)
                    stats = require_measurements(record)
                    if stats.get("complete") is not True or stats.get("cancelled") is not False:
                        raise ValueError(f"{case}/{name} did not complete the scan")
                    if case.startswith("source-"):
                        audit = report["fixtures"][case]["audit_before"]
                        expected_counts = {"entries": audit["entries_including_root"], "files": audit["files"],
                                           "directories": audit["directories_including_root"]}
                        if any(type(stats.get(key)) is not int or stats[key] != value for key, value in expected_counts.items()):
                            raise ValueError(f"{case}/{name} source coverage does not match the independently audited counts: expected {expected_counts}")
                        record["source_coverage_matches_fixture"] = True
                        record["expected_source_counts"] = expected_counts
                    proofs = helpers.eligible_proofs(output / record["stdout_artifact"], roots[case])
                    verify_eligible(proofs, report["fixtures"][case], manifests[case])
                    if bool(proofs) != (record["first_candidate_wall_ms"] is not None):
                        raise ValueError(f"{case}/{name} first-eligible timing disagrees with final eligibility")
                    if case not in reference:
                        reference[case] = proofs
                    if proofs != reference[case]:
                        raise ValueError(f"{case}/{name} eligible proof differs from its first baseline run")
                    record.update(eligible_candidates=proofs, eligible_sha256=json_digest(proofs), verified=True)
                    write_json(summary_path, report)
        completed = True
        if args.cancel_after_ms is not None:
            for round_number in range(1, args.warm_runs + 1):
                order = ("candidate", "baseline") if round_number % 2 else ("baseline", "candidate")
                for name in order:
                    record = run("source-deep", name, "cancel", round_number, ["--cancel-after-ms", str(args.cancel_after_ms)])
                    stats = require_measurements(record)
                    proofs = helpers.eligible_proofs(output / record["stdout_artifact"], roots["source-deep"])
                    verify_eligible(proofs, report["fixtures"]["source-deep"], manifests["source-deep"])
                    cancelled = stats.get("cancelled") is True and stats.get("complete") is False
                    finished = stats.get("cancelled") is False and stats.get("complete") is True
                    record.update(verified=cancelled,
                                  cancellation_status="cancelled" if cancelled else "not exercised: scan completed" if finished else "invalid terminal state",
                                  wall_minus_delay_upper_bound_ms=round(max(0, record["wall_ms"] - args.cancel_after_ms), 3))
                    write_json(summary_path, report)
    except (OSError, ValueError, KeyError, TypeError, KeyboardInterrupt) as error:
        report["failure"] = f"{type(error).__name__}: {error}"
        print(report["failure"], file=sys.stderr)
    finally:
        # Audit even failed/interrupted comparisons when a before-manifest exists.
        for case, expected in manifests.items():
            try:
                entries = manifest(roots[case])
                artifact = f"{case}.after-manifest.json"
                write_json(output / artifact, entries, exclusive=True)
                audit = audit_record(entries, artifact)
                audit["matches_before"] = entries == expected
                report["fixtures"][case]["audit_after"] = audit
                audit_ok = audit_ok and audit["matches_before"]
            except (OSError, ValueError) as error:
                audit_ok = False
                report["fixtures"][case]["audit_after"] = {"matches_before": False, "failure": str(error)}
        try:
            report["binaries_unchanged"] = all(sha256(path) == hashes[name] for name, path in binaries.items())
            report["harnesses_unchanged"] = all(sha256(path) == report["harnesses"][name]["sha256"] for name, path in tracked.items())
        except OSError as error:
            report["hash_verification_failure"] = str(error)
            report["binaries_unchanged"] = report["harnesses_unchanged"] = False
        report["verified"] = (completed and not report.get("failure") and len(manifests) == len(CASES)
                              and audit_ok and report["binaries_unchanged"] and report["harnesses_unchanged"])
        cancellation = report["cancellation"]
        cancellation["verified"] = (len(cancellation["runs"]) == args.warm_runs * 2
                                    and all(r.get("verified") for r in cancellation["runs"])) if cancellation["requested"] else None
        cancellation["summary"] = {}
        for name in ("baseline", "candidate"):
            samples = [r for r in cancellation["runs"] if r["tool"] == name]
            verified = [r for r in samples if r.get("verified")]
            cancellation["summary"][name] = {
                "verified_cancellations": len(verified), "total_runs": len(samples),
                "p95_wall_minus_delay_upper_bound_ms": benchmark.percentile95([r["wall_minus_delay_upper_bound_ms"] for r in verified]),
            }
        report["completed_at"] = datetime.now(timezone.utc).isoformat()
        report["summary"] = summarize(report["runs"])
        write_json(summary_path, report)
        (output / "REPORT.md").write_text(markdown_report(report), encoding="utf-8")
    print(summary_path)
    return 0 if report["verified"] and (not report["cancellation"]["requested"] or report["cancellation"]["verified"]) else 1


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, ValueError, KeyboardInterrupt) as error:
        print(f"Scanner matrix stopped: {error}", file=sys.stderr)
        raise SystemExit(1)
