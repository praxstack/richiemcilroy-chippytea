#!/usr/bin/env python3
"""Compare immutable Cargo discovery fixtures with ordinary and linked files.

Only the ordinary case is an equal-coverage performance control. Internal-link
eligibility is a new capability: a baseline which stops early does less work.
All fixtures and outputs are retained. This script never requests cleanup.
"""
from __future__ import annotations

import argparse
from datetime import datetime, timezone
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import shutil
import stat
import statistics
import sys
import time
import uuid

import benchmark

REPO = Path(__file__).resolve().parents[1]
MAGIC = "chippytea-hardlink-benchmark-v1"
CASES = ("ordinary", "internal", "external")
MODES = ("suggestions", "metadata")
TARGET = "project/target"
NOFOLLOW_ANY = 0x20000000  # Public macOS fcntl.h flag, absent from some Python builds.
RESERVE = 3 * 1024**3


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def digest(path: Path) -> str:
    value = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024**2), b""):
            value.update(block)
    return value.hexdigest()


def object_digest(value: object) -> str:
    return hashlib.sha256(json.dumps(value, sort_keys=True, separators=(",", ":")).encode()).hexdigest()


def metadata(info: os.stat_result) -> list[int]:
    return [info.st_dev, info.st_ino, info.st_mode, info.st_uid, info.st_gid,
            info.st_size, info.st_mtime_ns, info.st_ctime_ns, info.st_nlink,
            info.st_blocks * 512, info.st_flags]


def audit(fixture: Path) -> dict:
    """Hash content once per inode and retain metadata for every namespace name."""
    paths, contents, aliases = {}, {}, {}
    pending = [fixture]
    device = fixture.lstat().st_dev
    while pending:
        directory = pending.pop()
        info = directory.lstat()
        require(stat.S_ISDIR(info.st_mode) and info.st_uid == os.geteuid()
                and info.st_dev == device and info.st_flags == 0, "Unsafe fixture directory")
        paths[str(directory.relative_to(fixture))] = metadata(info)
        with os.scandir(directory) as entries:
            for entry in entries:
                path = Path(entry.path)
                info = entry.stat(follow_symlinks=False)
                require(info.st_uid == os.geteuid() and info.st_dev == device and info.st_flags == 0,
                        "Fixture ownership, device, or flags changed")
                if stat.S_ISDIR(info.st_mode):
                    pending.append(path)
                    continue
                require(stat.S_ISREG(info.st_mode), "Links and special files are not fixture inputs")
                relative, identity = str(path.relative_to(fixture)), f"{info.st_dev}:{info.st_ino}"
                values = metadata(info)
                paths[relative] = values
                aliases.setdefault(identity, []).append(relative)
                if identity in contents:
                    require(values == contents[identity]["metadata"], "Aliases disagree on inode metadata")
                    continue
                descriptor = os.open(path, os.O_RDONLY | os.O_CLOEXEC | NOFOLLOW_ANY)
                with os.fdopen(descriptor, "rb") as stream:
                    require(metadata(os.fstat(stream.fileno())) == values, "File changed before content audit")
                    value = hashlib.sha256()
                    for block in iter(lambda: stream.read(1024**2), b""):
                        value.update(block)
                    require(metadata(os.fstat(stream.fileno())) == values, "File changed during content audit")
                require(metadata(path.lstat()) == values, "File pathname changed during content audit")
                contents[identity] = {"metadata": values, "sha256": value.hexdigest()}
        require(metadata(directory.lstat()) == paths[str(directory.relative_to(fixture))], "Directory changed during audit")
    for identity, names in aliases.items():
        names.sort()
        require(contents[identity]["metadata"][8] == len(names), "A fixture file has an unaccounted external alias")
    return {"paths": paths, "inodes": contents, "aliases": aliases}


def make_fixture(groups: int) -> Path:
    budget = 310 * 1024**2 + groups * 12 * 8192
    require(shutil.disk_usage("/private/tmp").free >= RESERVE + budget,
            "Keep a 3 GiB reserve plus the payload and metadata budget")
    fixture = Path("/private/tmp") / ("chippytea-hardlinks-" + uuid.uuid4().hex)
    fixture.mkdir(mode=0o700)
    require(fixture.resolve(strict=True) == fixture, "Disposable parent must use its physical path")
    marker = fixture / ".chippytea-hardlink-fixture.json"
    marker.write_text(json.dumps({"magic": MAGIC, "status": "creating", "groups": groups}) + "\n")
    (fixture / "cargo-home").mkdir(mode=0o700)
    (fixture / "outside-authorized-roots").mkdir(mode=0o700)
    for case in CASES:
        root = fixture / case
        root.mkdir(mode=0o700)
        project, target = root / "project", root / TARGET
        (target / "a").mkdir(parents=True)
        (target / "b").mkdir()
        (project / "Cargo.toml").write_text('[package]\nname = "disposable-hardlinks"\nversion = "0.1.0"\nedition = "2021"\n\n[workspace]\n')
        (project / "Cargo.lock").write_text('version = 4\n\n[[package]]\nname = "disposable-hardlinks"\nversion = "0.1.0"\n')
        (target / "CACHEDIR.TAG").write_text("Signature: 8a477f597d28d172789f06886806bc55\n# Disposable build output.\n")
        with (target / "payload.bin").open("xb") as stream:
            block = b"P" * 1024**2
            for _ in range(100):
                stream.write(block)
            stream.flush()
            os.fsync(stream.fileno())
        for group in range(groups):
            original = target / "a" / f"group-{group:04}.bin"
            alias = target / "b" / original.name
            data = (group.to_bytes(4, "little") * 1024)
            with original.open("xb") as stream:
                stream.write(data)
            if case == "ordinary":
                with alias.open("xb") as stream:
                    stream.write(data)
            else:
                os.link(original, alias, follow_symlinks=False)
        if case == "external":
            os.link(target / "a/group-0000.bin", fixture / "outside-authorized-roots/preserved.bin", follow_symlinks=False)
    # Create all aliases before aging: link creation changes inode ctime.
    old = time.time_ns() - 9 * 86_400 * 1_000_000_000
    for case in CASES:
        for parent, _, files in os.walk(fixture / case, topdown=False, followlinks=False):
            for name in files:
                os.utime(Path(parent) / name, ns=(old, old), follow_symlinks=False)
            os.utime(parent, ns=(old, old), follow_symlinks=False)
    marker.write_text(json.dumps({"magic": MAGIC, "status": "complete", "groups": groups,
        "cases": CASES, "old_mtime_ns": old, "minimum_reserve_bytes": RESERVE,
        "external_alias": "outside-authorized-roots/preserved.bin"}, indent=2) + "\n")
    require(shutil.disk_usage("/private/tmp").free >= RESERVE, "Fixture creation exhausted its reserve")
    return fixture


def scope(evidence: dict, prefix: str) -> dict:
    paths = {name: value for name, value in evidence["paths"].items() if name == prefix or name.startswith(prefix + "/")}
    files = {name: value for name, value in paths.items() if stat.S_ISREG(value[2])}
    unique = {(value[0], value[1]): value for value in files.values()}
    return {"entries": len(paths), "files": len(files), "directories": len(paths) - len(files),
            "unique_regular_inodes": len(unique), "logical_bytes": sum(value[5] for value in unique.values()),
            "allocated_bytes": sum(value[9] for value in unique.values())}


def validate_shape(evidence: dict, groups: int) -> None:
    expected = {".", ".chippytea-hardlink-fixture.json", "cargo-home", "outside-authorized-roots", "outside-authorized-roots/preserved.bin"}
    for case in CASES:
        target = case + "/" + TARGET
        expected.update((case, case + "/project", case + "/project/Cargo.toml", case + "/project/Cargo.lock",
                         target, target + "/a", target + "/b", target + "/CACHEDIR.TAG", target + "/payload.bin"))
        for group in range(groups):
            first, second = (target + f"/{directory}/group-{group:04}.bin" for directory in ("a", "b"))
            expected.update((first, second))
            left, right = evidence["paths"][first], evidence["paths"][second]
            require(left[5] == right[5] == 4096 and stat.S_ISREG(left[2]) and stat.S_ISREG(right[2]), "Wrong link-group file shape")
            require((left[:2] == right[:2]) == (case != "ordinary"), "Link group does not have its declared identity relationship")
            links = 1 if case == "ordinary" else 3 if case == "external" and group == 0 else 2
            require(left[8] == right[8] == links, "Unexpected link count")
            names = evidence["aliases"][f"{left[0]}:{left[1]}"]
            allowed = {first} if case == "ordinary" else {first, second}
            if case == "external" and group == 0:
                allowed.add("outside-authorized-roots/preserved.bin")
            require(set(names) == allowed, "Link group crosses another fixture or an undeclared boundary")
        payload = evidence["paths"][target + "/payload.bin"]
        require(payload[5] == 100 * 1024**2 and payload[8] == 1, "Payload must be an independent 100 MiB file")
    require(set(evidence["paths"]) == expected, "Fixture contains unexpected entries")


def final_output(path: Path, expected_path: Path) -> tuple[dict, dict]:
    final, stats = {}, None
    with path.open("rb") as stream:
        while line := stream.readline(1024**2 + 1):
            require(len(line) <= 1024**2, "CLI JSON line exceeded 1 MiB")
            item = json.loads(line)
            require(isinstance(item, dict), "CLI must emit JSON objects")
            if "stats" in item:
                require(isinstance(item.get("candidates"), list), "Invalid candidate batch")
                for candidate in item["candidates"]:
                    require(isinstance(candidate, dict) and candidate.get("path") == str(expected_path), "Unexpected artifact output")
                    final[candidate["path"]] = candidate
            elif "entries" in item:
                stats = item
            else:
                raise ValueError("Unexpected CLI output shape")
    require(stats is not None and set(final) == {str(expected_path)}, "Missing final statistics or recognized Cargo diagnostic")
    return final[str(expected_path)], stats


def validate(record: dict, candidate: dict, stats: dict, evidence: dict, case: str, mode: str, variant: str) -> dict:
    require(record["exit_code"] == 0 and not record["timed_out"] and not record["json_line_exceeded_1_mib"], "CLI failed or exceeded output bounds")
    require(stats == record["engine_stats"] and stats["complete"] is True and not stats["cancelled"] and stats["errors"] == 0,
            "Discovery did not finish with complete safe coverage")
    for key in ("wall_ms", "user_cpu_seconds", "system_cpu_seconds", "peak_rss_bytes"):
        value = record[key]
        require(type(value) in (int, float) and math.isfinite(value) and value >= 0, "Missing CPU/wall/RSS observation")
    require(record["peak_rss_bytes"] > 0, "Missing process RSS")
    full, artifact = scope(evidence, case), scope(evidence, case + "/" + TARGET)
    expected_eligible = case == "ordinary" or (case == "internal" and variant == "candidate")
    require(candidate.get("kind") == "cargo" and candidate.get("provisional") is False
            and candidate.get("suggestion_eligible") is expected_eligible
            and candidate.get("eligible_permanent") is expected_eligible, "Unexpected Cargo eligibility")
    if expected_eligible:
        require(candidate["blocked_reason"] is None and candidate["fingerprint"]
                and record["first_candidate_wall_ms"] is not None, "Eligible artifact lacks complete proof or first-finding evidence")
    else:
        reason = candidate.get("blocked_reason")
        require(isinstance(reason, str) and "hard" in reason.lower() and "link" in reason.lower()
                and record["first_candidate_wall_ms"] is None, "Artifact was not withheld specifically for its hard links")
    require(stats["candidates"] == int(expected_eligible), "Final suggestion count disagrees with the artifact")
    identity = candidate["identity"]
    actual = evidence["paths"][case + "/" + TARGET]
    require([identity[key] for key in ("device", "inode", "mode", "size", "modified_ns", "changed_ns")]
            == [actual[index] for index in (0, 1, 2, 5, 6, 7)], "Artifact identity differs from independent audit")
    exhaustive = mode == "metadata" or case == "ordinary" or variant == "candidate"
    require(stats["entries"] == stats["files"] + stats["directories"]
            and full["entries"] - artifact["entries"] + 1 <= stats["entries"] <= full["entries"], "Invalid examined-entry accounting")
    if exhaustive:
        require(all(stats[key] == full[key] for key in ("entries", "files", "directories")),
                "Full path coverage differs from independent audit")
        require(candidate["file_count"] == artifact["files"] and candidate["logical_bytes"] == artifact["logical_bytes"]
                and candidate["allocated_bytes"] == artifact["allocated_bytes"], "Artifact paths or inode-deduplicated bytes are incorrect")
    else:
        require(0 <= candidate["file_count"] <= artifact["files"]
                and 0 <= candidate["logical_bytes"] <= artifact["logical_bytes"]
                and 0 <= candidate["allocated_bytes"] <= artifact["allocated_bytes"], "Partial diagnostic exceeds the artifact")
    # Suggestions deliberately counts the two evidence-file names without
    # fetching their metadata; only artifact measurement contributes bytes.
    byte_totals = full if mode == "metadata" else candidate
    require(all(stats[key] == byte_totals[key] for key in ("logical_bytes", "allocated_bytes")),
            "Unique-inode byte totals differ from the traversal policy")
    return {key: candidate[key] for key in ("path", "identity", "fingerprint", "evidence", "kind", "file_count",
            "logical_bytes", "allocated_bytes", "suggestion_eligible", "eligible_permanent", "blocked_reason")}


def summarize(records: list[dict]) -> dict:
    result = {}
    for case in CASES:
        result[case] = {}
        for mode in MODES:
            result[case][mode] = {}
            for variant in ("baseline", "candidate"):
                samples = [item for item in records if item["case"] == case and item["mode"] == mode
                           and item["variant"] == variant and item.get("verified")]
                warm = [item for item in samples if item["phase"] == "warm"]
                result[case][mode][variant] = {
                    "verified_runs": len(samples), "first_wall_ms": samples[0]["wall_ms"] if samples else None,
                    "warm_wall_median_ms": statistics.median(item["wall_ms"] for item in warm) if warm else None,
                    "warm_wall_p95_ms": benchmark.percentile95([item["wall_ms"] for item in warm]),
                    "warm_cpu_median_seconds": statistics.median(item["cpu_seconds"] for item in warm) if warm else None,
                    "warm_cpu_p95_seconds": benchmark.percentile95([item["cpu_seconds"] for item in warm]),
                    "maximum_process_peak_rss_bytes": max((item["peak_rss_bytes"] for item in samples), default=None),
                    "actual_entries": [item["engine_stats"]["entries"] for item in samples],
                    "eligible": [item["final_candidate"]["suggestion_eligible"] for item in samples],
                }
    return result


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline-cli", type=Path, required=True)
    parser.add_argument("--candidate-cli", type=Path, required=True)
    parser.add_argument("--groups", type=int, default=4096)
    parser.add_argument("--warm-runs", type=int, default=6)
    parser.add_argument("--timeout", type=float, default=120)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    require(platform.system() == "Darwin", "This harness requires macOS /usr/bin/time -l")
    require(1 <= args.groups <= 8192 and 1 <= args.warm_runs <= 100 and 1 <= args.timeout <= 3600, "Invalid fixture or run bounds")
    output = args.output.resolve()
    require((REPO / "benchmarks/local").resolve() in output.parents, "Use a new output directory under benchmarks/local")
    output.mkdir(parents=True, exist_ok=False)
    binaries = {name: getattr(args, name + "_cli").resolve(strict=True) for name in ("baseline", "candidate")}
    require(all(path.is_file() and os.access(path, os.X_OK) for path in binaries.values()), "Provide two existing executable CLI builds")
    report = {"protocol": 1, "verified": False, "started_at": datetime.now(timezone.utc).isoformat(),
        "hardware": benchmark.hardware(), "groups": args.groups, "warm_runs": args.warm_runs, "runs": [],
        "harness_sha256": digest(Path(__file__)), "helpers_sha256": digest(Path(benchmark.__file__)),
        "binaries": {name: {"path": str(path), "sha256": digest(path)} for name, path in binaries.items()},
        "method": "Public CLI scan with projects authorization, separately in Suggestions and explicit MetadataCoverage modes. Three immutable, independently audited Cargo fixtures: independent files, pairs wholly within one target, and one extra alias outside that authorized root. Each case has an independently written 100 MiB payload and fixed group0000 external witness. Only the ordinary case requires equal recommendation proof and exact coverage across builds and is a matched performance control. Baseline internal/external Suggestions may stop at the first hard link while candidate completes a containment proof; these capability timings and extra examined entries are not a like-for-like speedup. Whole-process monotonic wall, Darwin time user+system CPU, and process peak RSS include CLI startup/JSON output; RSS is not scanner-only memory and CPU has the tool's reported precision. Setup, audits and source/binary checks are outside timing. A separate first invocation and alternating warm pairs are retained for each case/mode. Creation and audits warm caches; no cold-cache claim. Metadata mode must cover every physical path and count allocated/logical regular bytes once per inode. Allocated bytes are estimated metadata, not physical recovery or coins. No index, cleanup, native UI, event ingestion, or real user files are exercised."}
    summary = output / "summary.json"
    fixture = None
    try:
        fixture = make_fixture(args.groups)
        report["fixture"] = str(fixture)
        initial = audit(fixture)
        validate_shape(initial, args.groups)
        (output / "audit-before.json").write_text(json.dumps(initial, sort_keys=True) + "\n")
        report["audit_before_sha256"] = object_digest(initial)
        report["physical_counts"] = {case: {"root": scope(initial, case), "artifact": scope(initial, case + "/" + TARGET)} for case in CASES}
        ordinary_proof, stable_proofs, full_proofs = None, {}, {}
        for case in CASES:
            for mode in MODES:
                for number in range(args.warm_runs + 1):
                    phase = "first" if number == 0 else "warm"
                    for variant in (("baseline", "candidate") if number % 2 == 0 else ("candidate", "baseline")):
                        require(shutil.disk_usage("/private/tmp").free >= RESERVE, "Disk reserve fell below 3 GiB")
                        require(digest(binaries[variant]) == report["binaries"][variant]["sha256"], "Frozen CLI changed")
                        command = ["/usr/bin/env", "-u", "CARGO_TARGET_DIR", "-u", "CARGO_BUILD_TARGET_DIR",
                                   "CARGO_HOME=" + str(fixture / "cargo-home"), str(binaries[variant]), "scan", str(fixture / case), "--kind", "projects"]
                        if mode == "metadata":
                            command.append("--metadata-coverage")
                        record = benchmark.run_sample(f"{case}-{mode}-{variant}", phase, number, command, output, args.timeout)
                        record.update(case=case, mode=mode, variant=variant, pair=number, verified=False)
                        report["runs"].append(record)
                        require(digest(binaries[variant]) == report["binaries"][variant]["sha256"], "CLI changed during invocation")
                        candidate, stats = final_output(output / record["stdout_artifact"], fixture / case / TARGET)
                        record["final_candidate"] = candidate
                        proof = validate(record, candidate, stats, initial, case, mode, variant)
                        key = (case, mode, variant)
                        require(stable_proofs.setdefault(key, proof) == proof, "A repeated scan changed its immutable artifact proof")
                        if mode == "metadata" or case == "ordinary" or variant == "candidate":
                            # Closure changes eligibility, not the full tree's
                            # digest, evidence, identity, or deduplicated sizes.
                            content_proof = {name: value for name, value in proof.items()
                                             if name not in ("suggestion_eligible", "eligible_permanent", "blocked_reason")}
                            require(full_proofs.setdefault(case, content_proof) == content_proof,
                                    "Complete scans disagree on artifact proof across builds or modes")
                        if case == "ordinary":
                            ordinary_proof = ordinary_proof or proof
                            require(proof == ordinary_proof, "The no-hardlink control changed its eligible proof across builds or modes")
                        record.update(cpu_seconds=record["user_cpu_seconds"] + record["system_cpu_seconds"],
                                      proof_sha256=object_digest(proof), verified=True)
                        summary.write_text(json.dumps(report, indent=2) + "\n")
                # This audit is outside every measured child-process interval.
                current = audit(fixture)
                require(current == initial, "Fixture contents, aliases, identities, or timestamps changed")
        report["audit_after_sha256"] = object_digest(current)
        require(digest(Path(__file__)) == report["harness_sha256"] and digest(Path(benchmark.__file__)) == report["helpers_sha256"], "Harness sources changed")
        report["verified"] = True
    except (Exception, KeyboardInterrupt) as error:
        report["failure"] = f"{type(error).__name__}: {error}"
        print(report["failure"], file=sys.stderr)
    finally:
        report["completed_at"] = datetime.now(timezone.utc).isoformat()
        report["summary"] = summarize(report["runs"])
        summary.write_text(json.dumps(report, indent=2) + "\n")
        if fixture is not None:
            print(f"Disposable fixture retained: {fixture}", file=sys.stderr)
    print(summary)
    return 0 if report["verified"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
