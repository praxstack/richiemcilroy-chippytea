#!/usr/bin/env python3
"""Compare recent-file events or repeated scans through the public Rust Engine API.

Uses marked disposable files only. Fixtures and libraries are retained. This
does not measure Swift, FFI, native filesystem event delivery, or Home scans.
"""
from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import math
import os
from pathlib import Path
import platform
import re
import shutil
import stat
import statistics
import subprocess
import time
import uuid

import benchmark

REPO = Path(__file__).resolve().parents[1]
DRIVER = REPO / "scripts/recent-file-benchmark.rs"
SAFETY = REPO / "core/src/safety.rs"
MAGIC = "chippytea-recent-file-v1\n"
ARTIFACT = "changed-project/node_modules"
CONTROL = "control-project/node_modules"
LEAF_BYTES = b"Old disposable generated output.\n"
# Public macOS fcntl.h flag; Python does not expose it in every bundled runtime.
NOFOLLOW_ANY = 0x20000000
SPEC = importlib.util.spec_from_file_location("event_benchmark_helpers", REPO / "scripts/benchmark-events.py")
HELPERS = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(HELPERS)
digest = HELPERS.digest


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def json_digest(value: object) -> str:
    return hashlib.sha256(json.dumps(value, sort_keys=True, separators=(",", ":")).encode()).hexdigest()


def audit(root: Path) -> dict:
    values = HELPERS.audit(root, include_allocated=True)
    for relative, record in values.items():
        info = (root / relative).lstat()
        require((info.st_dev, info.st_ino, info.st_mode, info.st_mtime_ns, info.st_ctime_ns)
                == (record[0], record[1], record[2], record[4], record[5]), "Tree changed during audit")
        require(info.st_flags == 0, "Fixture contains flagged or cloud-managed data")
        record.append(info.st_flags)
    return values


def guard_fixture(fixture: Path) -> None:
    require(fixture.parent == Path("/private/tmp") and re.fullmatch(r"chippytea-recent-file-[0-9a-f]{32}", fixture.name),
            "Use an exact /private/tmp/chippytea-recent-file-<32hex> fixture")
    require(fixture.resolve(strict=True) == fixture, "Fixture path must be physical")
    info = fixture.lstat()
    require(stat.S_ISDIR(info.st_mode) and stat.S_IMODE(info.st_mode) == 0o700
            and info.st_uid == os.geteuid() and info.st_flags == 0, "Fixture must be owned, local, and mode 0700")
    marker = fixture / ".chippytea-recent-fixture"
    with os.fdopen(os.open(marker, os.O_RDONLY | os.O_CLOEXEC | NOFOLLOW_ANY), "rb") as stream:
        info = os.fstat(stream.fileno())
        require(stat.S_ISREG(info.st_mode) and info.st_uid == os.geteuid() and info.st_nlink == 1
                and info.st_flags == 0 and stream.read(64) == MAGIC.encode(), "Missing exact disposable marker")
    root = fixture / "tree"
    require(root.resolve(strict=True) == root and stat.S_ISDIR(root.lstat().st_mode), "Scanned root is not physical")


def reserve(extra: int = 0) -> None:
    require(shutil.disk_usage("/private/tmp").free >= 3 * 1024**3 + extra,
            "Keep a 3 GiB disk reserve in addition to the fixture budget")


def make_fixture(leaves: int) -> tuple[Path, int]:
    reserve(210 * 1024**2 + (leaves + leaves // 256 + 32) * 8192)
    fixture = Path("/private/tmp") / ("chippytea-recent-file-" + uuid.uuid4().hex)
    fixture.mkdir(mode=0o700)
    (fixture / ".chippytea-recent-fixture").write_text(MAGIC)
    root = fixture / "tree"
    root.mkdir(mode=0o700)
    for name in ("changed-project", "control-project"):
        project = root / name
        artifact = project / "node_modules"
        artifact.mkdir(parents=True)
        (project / "package.json").write_text('{"name":"disposable","version":"1.0.0"}\n')
        (project / "package-lock.json").write_text('{"name":"disposable","lockfileVersion":3,"packages":{"":{"name":"disposable","version":"1.0.0"}}}\n')
        with (artifact / "payload.bin").open("xb") as stream:
            for _ in range(100):
                stream.write(b"x" * 1024**2)
            stream.flush()
            os.fsync(stream.fileno())
        if name == "changed-project":
            for group in range(leaves // 256):
                directory = artifact / "deep/nested" / f"group-{group:04}"
                directory.mkdir(parents=True)
                for member in range(256):
                    (directory / f"leaf-{member:04}.txt").write_bytes(LEAF_BYTES)
    old = time.time_ns() - 9 * 86_400 * 1_000_000_000
    for parent, _, files in os.walk(root, topdown=False):
        for name in files:
            os.utime(Path(parent) / name, ns=(old, old), follow_symlinks=False)
        os.utime(parent, ns=(old, old), follow_symlinks=False)
    return fixture, old


def compile_driver(archive: Path, name: str, output: Path, dependencies: Path) -> dict:
    archive_hash = digest(archive)
    copied = output / f"lib{name}_core.rlib"
    shutil.copyfile(archive, copied)
    require(digest(copied) == archive_hash, "Frozen rlib changed while copying")
    externs = {"chippytea_core": copied}
    for crate in ("libc", "blake3", "serde_json", "rusqlite"):
        matches = list(dependencies.glob(f"lib{crate}-*.rlib"))
        require(len(matches) == 1, f"Provide a matching --deps-dir with one {crate} rlib")
        externs[crate] = matches[0]
    hashes = {crate: digest(path) for crate, path in externs.items()}
    command = ["rustc", "--edition=2024", "-C", "opt-level=3", "-C", "debuginfo=0",
               "-L", f"dependency={dependencies}"]
    for crate, path in externs.items():
        command.extend(("--extern", f"{crate}={path}"))
    command.extend((str(DRIVER), "-o", str(output / (name + "-driver"))))
    result = subprocess.run(command, capture_output=True, text=True)
    (output / (name + "-build.log")).write_text(result.stdout + result.stderr)
    result.check_returncode()
    require(digest(archive) == archive_hash and all(digest(externs[key]) == value for key, value in hashes.items()),
            "An rlib changed during compilation")
    return {"command": command, "rlib_sha256": archive_hash, "extern_sha256": hashes,
            "driver_sha256": digest(output / (name + "-driver"))}


def validate_fixture(manifest: dict, evidence: dict) -> tuple[Path, int]:
    fixture = Path(manifest["fixture"])
    guard_fixture(fixture)
    leaves = manifest["files"] - 1
    require(isinstance(leaves, int) and 256 <= leaves <= 131_072 and leaves % 256 == 0, "Unexpected fixture leaf count")
    expected = {".", "changed-project", "control-project", ARTIFACT, CONTROL,
                ARTIFACT + "/deep", ARTIFACT + "/deep/nested", ARTIFACT + "/payload.bin", CONTROL + "/payload.bin"}
    for project in ("changed-project", "control-project"):
        expected.update((project + "/package.json", project + "/package-lock.json"))
    for group in range(leaves // 256):
        directory = ARTIFACT + f"/deep/nested/group-{group:04}"
        expected.add(directory)
        expected.update(directory + f"/leaf-{member:04}.txt" for member in range(256))
    require(set(evidence) == expected, "Fixture shape differs from the fixed production-order experiment")
    old = manifest["old_mtime_ns"]
    require(isinstance(old, int) and 0 < old < time.time_ns() - 8 * 86_400 * 1_000_000_000,
            "Fixture is not sufficiently old")
    require(all(value[4] == old for value in evidence.values()), "Every fixture mtime must start old")
    selected = manifest["selected"]
    require([value["ordinal"] for value in selected] == [1, leaves // 2, leaves]
            and len({value["path"] for value in selected}) == 3, "Witnesses must be fixed first/middle/last ordinals")
    artifact_members = {path: value for path, value in evidence.items() if path == ARTIFACT or path.startswith(ARTIFACT + "/")}
    require(manifest["entries"] == len(artifact_members), "Selection coverage differs from the physical artifact")
    for item in selected:
        path = Path(item["path"])
        relative = str(path.relative_to(fixture / "tree"))
        require(re.fullmatch(re.escape(ARTIFACT) + r"/deep/nested/group-[0-9]{4}/leaf-[0-9]{4}\.txt", relative)
                and relative in evidence and 1 <= item["entry"] <= len(artifact_members), "Selected leaf left the generated artifact")
        value = evidence[relative]
        require([item["identity"][key] for key in ("device", "inode", "mode", "size", "modified_ns")]
                == value[:5] and stat.S_ISREG(value[2]) and value[3] == len(LEAF_BYTES), "Frozen selection identity changed")
    return fixture, leaves


def read_leaf(path: Path, expected: list) -> bytes:
    with os.fdopen(os.open(path, os.O_RDONLY | os.O_CLOEXEC | NOFOLLOW_ANY), "rb") as stream:
        info = os.fstat(stream.fileno())
        require((info.st_dev, info.st_ino, info.st_mode, info.st_size, info.st_mtime_ns, info.st_ctime_ns)
                == tuple(expected[:6]) and info.st_uid == os.geteuid() and info.st_nlink == 1
                and info.st_flags == 0 and info.st_size == len(LEAF_BYTES), "Witness changed before byte audit")
        return stream.read(len(LEAF_BYTES) + 1)


def mutation_audit(before: dict, after: dict, relative: str, old_bytes: bytes, fixture: Path, mode: str, rescan: bool = False) -> None:
    mutates = mode == "recent" or rescan
    require(before.keys() == after.keys(), "The fixture gained or lost an entry")
    for path, value in before.items():
        if path != relative or not mutates:
            require(after[path] == value, "An unrequested fixture entry changed: " + path)
        else:
            require(all(after[path][index] == value[index] for index in (0, 1, 2, 3, 6, 7, 9)),
                    "Mutation changed witness identity, size, allocation, links, or flags")
            require((after[path][4] == value[4] if rescan and mode == "control" else after[path][4] > value[4])
                    and after[path][5] >= value[5], "Witness timestamps differ from the requested write or aging")
    expected = bytes([old_bytes[0] ^ 1]) + old_bytes[1:] if mutates else old_bytes
    require(read_leaf(fixture / "tree" / relative, after[relative]) == expected, "Mutation was not exactly one byte")


def restore_mtime(fixture: Path, relative: str, before: dict, after: dict, mode: str) -> dict:
    if mode == "control":
        return after
    path = fixture / "tree" / relative
    descriptor = os.open(path, os.O_RDONLY | os.O_CLOEXEC | NOFOLLOW_ANY)
    try:
        current = os.fstat(descriptor)
        expected = after[relative]
        require((current.st_dev, current.st_ino, current.st_mode, current.st_size, current.st_mtime_ns, current.st_ctime_ns)
                == tuple(expected[:6]) and current.st_uid == os.geteuid() and current.st_nlink == 1
                and current.st_flags == 0, "Witness changed before mtime restoration")
        os.utime(descriptor, ns=(current.st_atime_ns, before[relative][4]))
    finally:
        os.close(descriptor)
    restored = audit(fixture / "tree")
    require(restored.keys() == after.keys(), "Fixture shape changed during restoration")
    for name, value in after.items():
        if name != relative:
            require(restored[name] == value, "Restoration changed another fixture entry")
        else:
            require(all(restored[name][index] == value[index] for index in (0, 1, 2, 3, 6, 7, 8, 9))
                    and restored[name][4] == before[name][4] and restored[name][5] >= value[5],
                    "Only witness mtime and unavoidable ctime may change during restoration")
    return restored


def counts(evidence: dict) -> dict:
    return {"entries": len(evidence), "files": sum(stat.S_ISREG(value[2]) for value in evidence.values()),
            "directories": sum(stat.S_ISDIR(value[2]) for value in evidence.values())}


def validate(record: dict, fixture: Path, relative: str, evidence: dict, changed: dict, mode: str, rescan: bool = False) -> dict:
    require(record["protocol"] == (2 if rescan else 1) and record["mode"] == mode and record["fixture"] == str(fixture)
            and record["leaf"] == str(fixture / "tree" / relative), "Driver used another protocol or witness")
    before, after, full = (record[key] for key in ("before", "after", "full"))
    expected_request = {"action": "dirty", "root_id": before["roots"][0]["id"], "events": [
        {"path": str(fixture / "tree" / relative), "kind": "file", "recursive": False}]}
    action = "scan" if rescan else "dirty"
    require(record[action + "_request"] == ({"action": "scan"} if rescan else expected_request),
            "Measured request differs from the specified workload")
    full_counts = counts(evidence)
    members = {path: value for path, value in evidence.items() if path == ARTIFACT or path.startswith(ARTIFACT + "/")}
    artifact_counts = counts(members)
    expected_paths = {str(fixture / "tree" / path) for path in (ARTIFACT, CONTROL)}
    states = record["library_states"]
    initial = {value["path"]: value for value in states["before"]["candidates"]}
    require(set(initial) == expected_paths and len(before["candidates"]) == 2, "Initial index must contain exactly two eligible artifacts")
    control = initial[str(fixture / "tree" / CONTROL)]
    cursor = 0 if rescan else 1000
    stages = [("before", before, 0)]
    if rescan:
        stages.append(("seeded", record["seeded"], 0))
    stages.extend((("after", after, cursor), ("full", full, cursor)))
    for label, snapshot, cursor in stages:
        state = states[label]
        require(snapshot["scanning"] is False and snapshot["cleaning"] is False and snapshot["error"] is None
                and snapshot["stats"]["complete"] is True and not snapshot["stats"]["cancelled"] and snapshot["stats"]["errors"] == 0,
                "Endpoint is active, incomplete, or failed")
        require(snapshot["roots"] == before["roots"] and len(snapshot["roots"]) == 1
                and snapshot["roots"][0]["path"] == str(fixture / "tree"), "Authorization changed")
        require(all(value == (1 if key in ("roots", "scans") else 0) for key, value in state["counts"].items())
                and state["wallet"] == [0, 0, 0] and state["cursor"] == cursor
                and snapshot["wallet"] == {"collected_coins": 0, "pending_coins": 0, "fractional_bytes": 0, "credited_bytes": 0}
                and not snapshot["history"] and not snapshot["kept_paths"], "Journal, coverage, cursor, or ledger audit failed")
        foreground = snapshot["foreground_scan"]
        require(foreground is not None and foreground["active"] is False and foreground["stats"]["complete"] is True
                and not foreground["stats"]["cancelled"] and foreground["stats"]["errors"] == 0
                and foreground == state["foreground_scan"], "Saved foreground is not complete and durable")
        if not rescan or label in ("before", "full"):
            require(all(foreground["stats"][key] == value for key, value in full_counts.items()), "Saved foreground lost whole-root counts")
        scope = state["scope_stats"]
        require(scope["complete"] is True and not scope["cancelled"] and scope["errors"] == 0, "Scope was not completed")
        indexed = {value["path"]: value for value in state["candidates"]}
        require(len(state["candidates"]) == 2 and set(indexed) == expected_paths and indexed[control["path"]] == control,
                "Independent control or indexed artifact set changed")
        affected = indexed[str(fixture / "tree" / ARTIFACT)]
        if label == "before" or (mode == "control" and label != "seeded"):
            if not rescan or label == "before":
                require(indexed == initial, "An unchanged eligible proof differs from the initial scan")
            require(all(value["suggestion_eligible"] and value["eligible_permanent"]
                    and not value["provisional"] and value["blocked_reason"] is None for value in indexed.values()),
                    "Old-file event or initial scan changed an eligible proof")
        else:
            require(not affected["suggestion_eligible"] and not affected["eligible_permanent"] and not affected["provisional"],
                    "Recent artifact remained eligible")
            if rescan and label in ("seeded", "after"):
                require(affected["modified_ns"] == record["mutation"]["seeded"]["modified_ns"]
                        and "quiet" in (affected["blocked_reason"] or ""),
                        "Suggestion scan did not discover the chosen recent witness")
        require({value["path"] for value in snapshot["candidates"]} == {path for path, value in indexed.items() if value["suggestion_eligible"]},
                "Displayed recommendations disagree with the index")
        if label in ("before", "full") or (rescan and label == "after" and mode == "control"):
            require(all(snapshot["stats"][key] == value and scope[key] == value for key, value in full_counts.items()),
                    "Initial or explicit MetadataCoverage scan missed physical entries")
            for path, item in indexed.items():
                artifact = str(Path(path).relative_to(fixture / "tree"))
                files = [value for name, value in evidence.items() if name.startswith(artifact + "/") and stat.S_ISREG(value[2])]
                require(item["logical_bytes"] == sum(value[3] for value in files)
                        and item["allocated_bytes"] == sum(value[7] for value in files) and item["file_count"] == len(files),
                        "Exhaustive artifact measurement disagrees with independent physical audit")
    if not rescan:
        require(after["foreground_scan"] == before["foreground_scan"], "Background event changed the saved foreground")
    scope = states["after"]["scope_stats"]
    if rescan:
        require(all(after["stats"][key] == scope[key] == after["foreground_scan"]["stats"][key] for key in full_counts)
                and scope["entries"] == scope["files"] + scope["directories"]
                and full_counts["entries"] - artifact_counts["entries"] + 1 <= scope["entries"] <= full_counts["entries"],
                "Repeated scan failed to retain whole-root coverage outside the excluded artifact")
        if mode == "control":
            require(states["after"]["candidates"] == states["full"]["candidates"] and after["candidates"] == full["candidates"],
                    "Aged-witness fallback differs from fresh exhaustive candidate proofs")
        seeded = record["mutation"]["seeded"]
        require(seeded["modified_ns"] > evidence[relative][4]
                and all(seeded[key] == record["mutation"]["after"][key]
                        for key in ("device", "inode", "mode", "size", "uid", "links", "flags", "allocated_bytes")),
                "Learned witness was not a recent file with the same retained identity and allocation")
    else:
        require(all(after["stats"][key] - before["stats"][key] == scope[key] for key in full_counts)
                and scope["entries"] == scope["files"] + scope["directories"]
                and 1 <= scope["entries"] <= artifact_counts["entries"], "Incremental counts do not describe one bounded artifact refresh")
        if mode == "control":
            require(all(scope[key] == value for key, value in artifact_counts.items()), "Old witness did not exercise complete fallback traversal")
    for label, audit_values in (("before", evidence), ("after", changed)):
        actual = record["mutation"][label]
        value = audit_values[relative]
        require([actual[key] for key in ("device", "inode", "mode", "size", "modified_ns", "changed_ns", "links", "allocated_bytes")]
                == value[:8] and actual["uid"] == os.geteuid() and actual["flags"] == value[9], "Driver mutation metadata differs from independent audit")
    for key in ("wall_seconds", "cpu_seconds", action + "_acknowledgment_seconds", action + "_acknowledgment_cpu_seconds"):
        require(type(record[key]) in (int, float) and math.isfinite(record[key]) and record[key] >= 0, "Invalid measurement")
    require(record[action + "_acknowledgment_seconds"] <= record["wall_seconds"]
            and record[action + "_acknowledgment_cpu_seconds"] <= record["cpu_seconds"] and record["lifetime_peak_rss_bytes"] > 0,
            "Acknowledgment or memory boundary is invalid")
    proof = {"scope_counts": {key: scope[key] for key in full_counts}, "full_counts": full_counts,
             "control_proof_sha256": json_digest(control), "endpoint_index_sha256": json_digest(states["after"]["candidates"])}
    if rescan:
        proof["untimed_seed_counts"] = {key: states["seeded"]["scope_stats"][key] for key in full_counts}
    return proof


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--before-rlib", type=Path, required=True)
    parser.add_argument("--after-rlib", type=Path, required=True)
    parser.add_argument("--deps-dir", type=Path, default=REPO / "target/release/deps")
    parser.add_argument("--fixture-manifest", type=Path, help="Reuse an owned marked fixture and its previously frozen ordinal selection")
    parser.add_argument("--leaves", type=int, default=32768, help="Fresh fixture leaves, in groups of 256")
    parser.add_argument("--warm-runs", type=int, default=10)
    parser.add_argument("--workload", choices=("event", "rescan"), default="event",
                        help="rescan learns a recent-file hint in an untimed full scan, then times the next full scan; its control ages that witness first")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    require(platform.system() == "Darwin", "This benchmark requires macOS 14 or later")
    require(256 <= args.leaves <= 131_072 and args.leaves % 256 == 0 and 1 <= args.warm_runs <= 100, "Invalid fixture or repetition count")
    output = args.output.resolve()
    require((REPO / "benchmarks/local").resolve() in output.parents, "Use a new output directory under benchmarks/local")
    output.mkdir(parents=True, exist_ok=False)
    sources = {str(path.relative_to(REPO)): digest(path) for path in (Path(__file__), DRIVER, SAFETY, REPO / "scripts/benchmark-events.py", REPO / "scripts/benchmark.py")}
    rescan = args.workload == "rescan"
    action = "scan" if rescan else "dirty"
    timing_keys = ("wall_seconds", "cpu_seconds", action + "_acknowledgment_seconds", action + "_acknowledgment_cpu_seconds", "lifetime_peak_rss_bytes")
    report = {"protocol": 2 if rescan else 1, "workload": args.workload, "verified": False, "hardware": benchmark.hardware(), "source_sha256": sources, "runs": [],
        "method": "Public Rust Engine API, unchanged driver linked against each frozen rlib. No Swift, FFI, native watcher, or profiler. An untimed production metadata traversal selects fixed first/middle/last leaf ordinals, or a supplied manifest retains an earlier selection. Each invocation uses a fresh library and initial two-artifact full scan. A verified existing leaf is toggled by one byte and fsynced before timing, retaining its inode, size, allocation, and every ancestor mtime; the old-file control does not write. Dirty acknowledgment ends before cursor receipt. Total process CPU/wall include the typed file event, durable cursor, normal 600 ms background debounce, and 5 ms settled snapshot polling. Complete index, foreground, journal and zero-ledger proof is captured and checked before an untimed explicit MetadataCoverage full scan. Full physical audits bracket every invocation and mtime restoration, retaining compact digests/deltas. Ctime cannot be restored; the toggled content is retained and becomes the next invocation's audited baseline. RSS is process lifetime through the measured endpoint, including initial scanning. First invocations are reported separately from alternating warm pairs; no cold-cache or Home improvement claim. Partial diagnostic bytes/fingerprints need not agree between pruning strategies; eligible control proof, exclusions, full coverage and exhaustive measurements must agree."}
    if rescan:
        report["method"] = "Public Rust Engine API, unchanged driver against each frozen rlib. No Swift, FFI, watcher, file-event requests, profiler or cleanup. Fixed first/middle/last leaf ordinals in an aged artifact and an unchanged eligible control. Each invocation uses a fresh library and initial eligible scan, toggles one byte of the selected existing leaf, then performs an untimed full suggestion scan to discover the recent leaf. The timed interval is the next explicit full suggestion Scan request through settlement, including durable journal/foreground writes and 5 ms snapshot polling. The aged control restores the learned leaf's original mtime before timing, without delivering an event; it must fall back to full eligible discovery. Timings exclude setup, the learning scan, aging and an independent exhaustive MetadataCoverage verification after the measured endpoint. Full physical audits bracket each invocation and restoration; only one leaf byte and its mtime/ctime may change. Complete index, authorization, coverage, foreground, journal and zero-ledger proofs are checked before exhaustive verification. RSS is process lifetime through the timed endpoint, including initial and learning scans. One separate first pair precedes alternating warm pairs. No cold-cache, idle CPU, window or Home scan claim. Partial ineligible measurements may differ; eligible control proofs, exclusions and exhaustive measurements must agree."
    summary = output / "summary.json"
    try:
        report["builds"] = {name: compile_driver(getattr(args, name + "_rlib").resolve(strict=True), name, output, args.deps_dir.resolve(strict=True))
                            for name in ("before", "after")}
        require(all(digest(REPO / path) == value for path, value in sources.items()), "Sources changed while building common drivers")
        if args.fixture_manifest:
            manifest = json.loads(args.fixture_manifest.read_text())
            fixture = Path(manifest["fixture"])
            guard_fixture(fixture)
        else:
            fixture, old = make_fixture(args.leaves)
            result = subprocess.run([str(output / "before-driver"), "select", str(fixture), str(args.leaves)],
                                    capture_output=True, text=True, timeout=120)
            (output / "selection.stdout.json").write_text(result.stdout)
            (output / "selection.stderr.log").write_text(result.stderr)
            result.check_returncode()
            manifest = json.loads(result.stdout)
            manifest.update(fixture=str(fixture), old_mtime_ns=old)
        initial_audit = audit(fixture / "tree")
        fixture, leaves = validate_fixture(manifest, initial_audit)
        (output / "fixture.json").write_text(json.dumps(manifest, indent=2) + "\n")
        report.update(fixture=str(fixture), selection=manifest, leaves=leaves,
                      initial_tree_sha256=json_digest(initial_audit), physical_counts=counts(initial_audit))
        cases = list(zip(("first", "middle", "last"), manifest["selected"]))
        cases.append(("aged" if rescan else "unchanged", manifest["selected"][1]))
        serial = 0
        control_proof = None
        previous = initial_audit
        for case, selected in cases:
            relative = str(Path(selected["path"]).relative_to(fixture / "tree"))
            mode = "control" if case in ("unchanged", "aged") else "recent"
            for number in range(args.warm_runs + 1):
                for name in (("before", "after") if number % 2 == 0 else ("after", "before")):
                    reserve()
                    serial += 1
                    driver = output / (name + "-driver")
                    require(digest(driver) == report["builds"][name]["driver_sha256"], "Frozen driver changed")
                    before = audit(fixture / "tree")
                    require(before == previous, "Fixture changed between invocations")
                    old_bytes = read_leaf(fixture / "tree" / relative, before[relative])
                    run_id = uuid.uuid4().hex
                    prefix = f"{serial:03}-{case}-{name}"
                    command = [str(driver), "rescan" if rescan else "run", str(fixture), "tree/" + relative, run_id, mode, str(manifest["old_mtime_ns"])]
                    result = subprocess.run(command, capture_output=True, text=True, timeout=180)
                    (output / (prefix + ".stdout.json")).write_text(result.stdout)
                    (output / (prefix + ".stderr.log")).write_text(result.stderr)
                    after = audit(fixture / "tree")
                    audit_record = {"before_sha256": json_digest(before), "after_sha256": json_digest(after),
                                    "leaf": relative, "leaf_before": before[relative], "leaf_after": after.get(relative)}
                    (output / (prefix + ".audit.json")).write_text(json.dumps(audit_record, indent=2) + "\n")
                    result.check_returncode()
                    record = json.loads(result.stdout)
                    require(record["database"] == str(fixture / f"state-{run_id}.sqlite"), "Driver used another library")
                    mutation_audit(before, after, relative, old_bytes, fixture, mode, rescan)
                    proof = validate(record, fixture, relative, before, after, mode, rescan)
                    control_proof = control_proof or proof["control_proof_sha256"]
                    require(proof["control_proof_sha256"] == control_proof, "Unchanged control proof differs across invocations or builds")
                    previous = restore_mtime(fixture, relative, before, after, mode)
                    audit_record.update(restored_sha256=json_digest(previous), leaf_restored=previous[relative],
                                        ctime_restored=False, verified=True)
                    (output / (prefix + ".audit.json")).write_text(json.dumps(audit_record, indent=2) + "\n")
                    retained = {key: record[key] for key in timing_keys}
                    retained.update(case=case, variant=name, phase="first" if number == 0 else "warm", pair=number,
                                    ordinal=selected["ordinal"], selection_entry=selected["entry"], run_id=run_id,
                                    evidence_prefix=prefix, proof=proof, audit=audit_record, verified=True)
                    report["runs"].append(retained)
                    summary.write_text(json.dumps(report, indent=2) + "\n")
        report["warm_summary"] = {}
        for case, _ in cases:
            report["warm_summary"][case] = {}
            for name in ("before", "after"):
                samples = [run for run in report["runs"] if run["case"] == case and run["variant"] == name and run["phase"] == "warm"]
                report["warm_summary"][case][name] = {key: {"median": statistics.median(run[key] for run in samples),
                    "p95": benchmark.percentile95([run[key] for run in samples])}
                    for key in timing_keys}
                report["warm_summary"][case][name]["actual_scope_entries"] = [run["proof"]["scope_counts"]["entries"] for run in samples]
                if rescan:
                    report["warm_summary"][case][name]["untimed_seed_entries"] = [run["proof"]["untimed_seed_counts"]["entries"] for run in samples]
        require(all(digest(REPO / path) == value for path, value in sources.items()), "Common harness sources changed during comparison")
        report["verified"] = True
    except Exception as error:
        report["error"] = str(error)
        raise
    finally:
        summary.write_text(json.dumps(report, indent=2) + "\n")
    print(summary)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
