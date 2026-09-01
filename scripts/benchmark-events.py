#!/usr/bin/env python3
"""Compare durable event processing on freshly created, disposable libraries.

Both archives must come from builds of this repository. No existing files are
removed. The scanned tree is shared and independently audited; each invocation
gets a new database. Driver output includes the full before/after snapshots.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import shutil
import sqlite3
import stat
import statistics
import subprocess
import time
import uuid

import benchmark

REPO = Path(__file__).resolve().parent.parent
MAGIC = "chippytea-event-benchmark-v1\n"
REPLAY = "unindexed-artifact-replay"
PERIODIC = "periodic-background"
ARTIFACT_BATCH = "artifact-event-batch"
CARGO_LOCK = "cargo-lock-refresh"
ARTIFACT = "preserved-project/node_modules"
CARGO_PROJECT = "cargo-project"
CARGO_TARGET = CARGO_PROJECT + "/target"
SOURCE_FILES_PER_DIRECTORY = 16


def digest(path: Path) -> str:
    value = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            value.update(block)
    return value.hexdigest()


def audit(root: Path, *, include_allocated: bool = False) -> dict:
    result = {}
    for parent, directories, files in os.walk(root, followlinks=False):
        for path in [Path(parent), *(Path(parent) / name for name in files)]:
            info = path.lstat()
            if path.is_symlink() or info.st_uid != os.geteuid():
                raise ValueError("The disposable tree changed ownership or contains a symlink")
            record = [info.st_dev, info.st_ino, info.st_mode, info.st_size,
                      info.st_mtime_ns, info.st_ctime_ns, info.st_nlink]
            if include_allocated:
                record.append(info.st_blocks * 512)
            if path.is_file():
                if info.st_nlink != 1:
                    raise ValueError("Fixture files must not be shared through hard links")
                record.append(digest(path))
            elif not path.is_dir():
                raise ValueError("Unexpected special file in fixture")
            result[str(path.relative_to(root))] = record
        if any((Path(parent) / name).is_symlink() for name in directories):
            raise ValueError("A fixture directory was replaced by a symlink")
    return result


def make_fixture(scopes: int, workload: str, artifact_files: int) -> tuple[Path, dict]:
    extra = (artifact_files if workload == REPLAY else scopes if workload == ARTIFACT_BATCH else 0) * 8192
    if workload == CARGO_LOCK:
        extra += (scopes * (SOURCE_FILES_PER_DIRECTORY + 1) + 7) * 8192
    if shutil.disk_usage("/private/tmp").free < 3 * 1024**3 + 110 * 1024**2 + extra:
        raise ValueError("The fixture needs a 3 GiB reserve plus its payload and file metadata budget")
    fixture = Path("/private/tmp") / ("chippytea-event-benchmark-" + uuid.uuid4().hex)
    fixture.mkdir(mode=0o700)
    (fixture / ".chippytea-event-fixture").write_text(MAGIC)
    root = fixture / "baseline"
    root.mkdir(mode=0o700)
    project = root / "preserved-project"
    artifact = project / "node_modules"
    artifact.mkdir(parents=True)
    (project / "package.json").write_text('{"name":"disposable","version":"1.0.0"}\n')
    (project / "package-lock.json").write_text('{"name":"disposable","lockfileVersion":3,"packages":{"":{"name":"disposable","version":"1.0.0"}}}\n')
    with (artifact / "payload.bin").open("xb") as stream:
        block = b"\x32" * 1024**2
        for _ in range(100):
            stream.write(block)
        stream.flush()
        os.fsync(stream.fileno())
    if workload == REPLAY:
        for index in range(artifact_files):
            directory = artifact / f"group-{index // 64:04}"
            directory.mkdir(exist_ok=True)
            (directory / f"file-{index % 64:04}.txt").write_text("Preserve this disposable artifact member.\n")
    elif workload == ARTIFACT_BATCH:
        for index in range(scopes):
            (artifact / f"event-{index:04}.txt").write_text("Preserve this disposable event target.\n")
    old = time.time_ns() - 9 * 86_400 * 1_000_000_000
    for parent, _, files in os.walk(artifact, topdown=False, followlinks=False):
        for name in files:
            os.utime(Path(parent) / name, ns=(old, old), follow_symlinks=False)
        os.utime(parent, ns=(old, old), follow_symlinks=False)
    for path in [project / "package.json", project / "package-lock.json", project]:
        os.utime(path, ns=(old, old), follow_symlinks=False)
    if workload == "mixed":
        for index in range(scopes // 2):
            directory = root / f"existing-{index:04}"
            directory.mkdir()
            (directory / "source.txt").write_text("Preserve this disposable source.\n")
    elif workload == PERIODIC:
        directory = root / "periodic-source"
        directory.mkdir()
        (directory / "source.txt").write_text("Preserve this disposable source.\n")
    elif workload == CARGO_LOCK:
        cargo = root / CARGO_PROJECT
        target = root / CARGO_TARGET
        target.mkdir(parents=True)
        (cargo / "Cargo.toml").write_text('[package]\nname = "event-fixture"\nversion = "0.1.0"\nedition = "2021"\n\n[workspace]\n')
        (cargo / "Cargo.lock").write_text('version = 4\n\n[[package]]\nname = "event-fixture"\nversion = "0.1.0"\n')
        (target / "CACHEDIR.TAG").write_text("Signature: 8a477f597d28d172789f06886806bc55\n# Disposable Cargo output.\n")
        (target / "payload.bin").write_bytes(b"Preserve this disposable Cargo output.\n" * 128)
        for index in range(scopes):
            directory = cargo / f"source-{index:04}"
            directory.mkdir()
            for member in range(SOURCE_FILES_PER_DIRECTORY):
                (directory / f"unit-{member:04}.rs").write_text("// Preserve this unrelated disposable source.\n")
        for parent, _, files in os.walk(cargo, topdown=False, followlinks=False):
            for name in files:
                os.utime(Path(parent) / name, ns=(old, old), follow_symlinks=False)
            os.utime(parent, ns=(old, old), follow_symlinks=False)
        # The child process uses this empty home so unrelated user Cargo
        # configuration cannot change the synthetic target's ownership policy.
        (fixture / "cargo-home").mkdir(mode=0o700)
    evidence = audit(root, include_allocated=workload in (ARTIFACT_BATCH, CARGO_LOCK))
    (fixture / "fixture.json").write_text(json.dumps({"magic": MAGIC.strip(), "scopes": scopes,
        "workload": workload, "expected_eligible": ARTIFACT, "audit": evidence}, indent=2) + "\n")
    return fixture, evidence


def compile_driver(archive: Path, output: Path) -> dict:
    archive_hash = digest(archive)
    command = ["xcrun", "clang", "-O3", "-std=c11", "-Wall", "-Wextra", "-Werror",
        "-mmacosx-version-min=14.0", "-I", str(REPO / "native/Bridge"),
        str(REPO / "scripts/event-benchmark.c"), str(archive), "-lsqlite3",
        "-framework", "Security", "-framework", "CoreServices", "-o", str(output)]
    result = subprocess.run(command, capture_output=True, text=True)
    output.with_suffix(".build.log").write_text(result.stdout + result.stderr)
    result.check_returncode()
    if digest(archive) != archive_hash:
        raise ValueError("Rust archive changed while the driver was being linked")
    return {"command": command, "archive_sha256": archive_hash, "driver_sha256": digest(output)}


def validate_periodic(record: dict, fixture: Path, evidence: dict, snapshots: list[dict]) -> None:
    before, after, full = snapshots
    scope = "periodic-source"
    members = {path for path in evidence if path == scope or path.startswith(scope + "/")}
    if (members != {scope, scope + "/source.txt"} or not stat.S_ISDIR(evidence[scope][2])
            or not stat.S_ISREG(evidence[scope + "/source.txt"][2])):
        raise ValueError("Periodic scope must contain exactly one directory and one file")
    examined = after["stats"]["entries"] - before["stats"]["entries"]
    if examined % 2 or not 1 <= examined // 2 <= 16:
        raise ValueError("Periodic entry delta does not describe complete bounded scope traversals")
    passes = examined // 2
    if any(after["stats"][key] - before["stats"][key] != passes for key in ("files", "directories")):
        raise ValueError("Periodic file/directory counts disagree with completed scope traversals")
    if (before["stats"]["entries"] != len(evidence)
            or before["foreground_scan"] != after["foreground_scan"]
            or any(snapshot["candidates"] != before["candidates"] for snapshot in snapshots)):
        raise ValueError("Periodic work changed foreground coverage or the positive recommendation")
    if (record.get("pre_probe_queue_empty") is not True or record.get("period_seconds") != 0.4
            or record.get("window_seconds") != 8 or record.get("maximum_lateness_seconds") != 0.05
            or record.get("event_path") != str(fixture / "baseline" / scope)
            or record.get("event_kind") != "directory" or record.get("event_recursive") is not True):
        raise ValueError("Periodic workload did not use the fixed observation protocol")

    def seconds(container: dict, key: str) -> float:
        value = container[key]
        if not isinstance(value, (int, float)) or isinstance(value, bool) or not math.isfinite(value) or value < 0:
            raise ValueError("Invalid periodic timestamp: " + key)
        return value

    events = record["events"]
    if len(events) != 16:
        raise ValueError("Periodic workload must submit exactly sixteen events")
    epsilon = 1e-8  # Driver timestamps retain nine decimal places.
    for index, event in enumerate(events):
        scheduled = seconds(event, "scheduled_seconds")
        submitted = seconds(event, "submitted_seconds")
        acknowledged = seconds(event, "acknowledged_seconds")
        cursor_ack = seconds(event, "cursor_acknowledged_seconds")
        if (abs(scheduled - index * 0.4) > epsilon or event["cursor"] != 1001 + index
                or not scheduled - epsilon <= submitted <= acknowledged <= cursor_ack
                or cursor_ack > scheduled + 0.05 + epsilon):
            raise ValueError("Periodic input missed its fixed delivery or receipt budget")
    endpoint = seconds(record, "endpoint_requested_seconds")
    elapsed = seconds(record, "wall_seconds")
    process_cpu = seconds(record, "cpu_seconds")
    if not 8 - epsilon <= endpoint <= elapsed <= 8.05 + epsilon:
        raise ValueError("Periodic CPU measurement did not use the fixed eight-second window")
    if not 0 < process_cpu <= elapsed * (os.cpu_count() or 1):
        raise ValueError("Periodic process CPU lies outside the observation window's capacity")

    probe = record["probe"]
    if (probe.get("exact_pending_scope") is not True or probe.get("final_queue_empty") is not True
            or probe.get("cursor") != 1017
            or probe.get("pending_path") != str(fixture / "baseline" / scope)):
        raise ValueError("Scan probe did not prove its exact pending input and final queue")
    event_ack = seconds(probe, "event_acknowledged_seconds")
    cursor_ack = seconds(probe, "cursor_acknowledged_seconds")
    gate_started = seconds(probe, "gate_started_seconds")
    scan_submitted = seconds(probe, "scan_submitted_seconds")
    scan_ack = seconds(probe, "scan_acknowledgment_seconds")
    scan_completed = seconds(probe, "scan_completion_seconds")
    if (not event_ack <= cursor_ack <= 0.05 + epsilon
            or not 0.05 - epsilon <= gate_started <= scan_submitted <= 0.10 + epsilon
            or not scan_ack <= scan_completed <= 30.05):
        raise ValueError("Scan probe missed the pending-event window or returned invalid latency")
    if probe["gate"].get("ok") is not True:
        raise ValueError("Scan probe did not receive a successful gate snapshot")
    gate = probe["gate"]["data"]
    if (gate["scanning"] is not True or gate["cleaning"] or gate["error"]
            or gate["stats"]["errors"] or gate["stats"]["cancelled"]
            or any(gate[key] != after[key] for key in ("roots", "candidates", "wallet", "history", "kept_paths", "foreground_scan"))
            or any(gate["stats"][key] != after["stats"][key] for key in ("entries", "files", "directories"))):
        raise ValueError("Scan probe was not waiting behind an untouched background scope")
    if (full["stats"]["entries"] != after["stats"]["entries"] + len(evidence)
            or any(full["stats"][key] != after["stats"][key] + before["stats"][key] for key in ("files", "directories"))):
        raise ValueError("Scan probe did not replace the pending child with one complete root traversal")
    record["completed_scope_passes"] = passes
    record["scope_entries"] = 2


def validate_artifact_batch(record: dict, fixture: Path, scopes: int,
                            evidence: dict, snapshots: list[dict]) -> None:
    before, after = snapshots
    members = {path: value for path, value in evidence.items()
               if path == ARTIFACT or path.startswith(ARTIFACT + "/")}
    expected_files = {ARTIFACT + "/payload.bin", *(ARTIFACT + f"/event-{index:04}.txt" for index in range(scopes))}
    if (set(members) != {ARTIFACT, *expected_files} or not stat.S_ISDIR(members[ARTIFACT][2])
            or any(not stat.S_ISREG(members[path][2]) for path in expected_files)):
        raise ValueError("Artifact batch must target the exact distinct generated regular files")
    events = [{"path": str(fixture / "baseline" / ARTIFACT / f"event-{index:04}.txt"),
               "kind": "file", "recursive": False} for index in range(scopes)]
    if (record.get("event_paths_verified") is not True
            or record.get("dirty_request") != {"action": "dirty", "root_id": before["roots"][0]["id"], "events": events}):
        raise ValueError("Artifact batch did not submit the exact existing typed file paths in one request")
    if (any(snapshot["stats"]["complete"] is not True for snapshot in snapshots)
            or before["stats"]["entries"] != len(evidence)
            or before["foreground_scan"] != after["foreground_scan"]
            or before["candidates"] != after["candidates"]):
        raise ValueError("Artifact maintenance changed full coverage, saved foreground or candidate proof")
    counts = {"entries": len(members), "files": len(expected_files), "directories": 1}
    if any(after["stats"][key] - before["stats"][key] != count for key, count in counts.items()):
        raise ValueError("Both variants must perform exactly one complete artifact traversal")
    for key in ("dirty_acknowledgment_seconds", "dirty_acknowledgment_cpu_seconds"):
        value = record[key]
        if isinstance(value, bool) or not isinstance(value, (int, float)) or not math.isfinite(value) or value < 0:
            raise ValueError("Invalid dirty acknowledgment measurement: " + key)
    if (record["dirty_acknowledgment_seconds"] > record["wall_seconds"] + 1e-8
            or record["dirty_acknowledgment_cpu_seconds"] > record["cpu_seconds"] + 1e-8):
        raise ValueError("Dirty acknowledgment must lie within the settled measurement")
    record["artifact_entries"] = len(members)
    record["artifact_passes"] = 1
    record["event_count"] = scopes


def validate_cargo_lock(record: dict, fixture: Path, scopes: int, evidence: dict,
                        snapshots: list[dict], variant: str) -> list[dict]:
    before, after, full = snapshots
    source_directories = {CARGO_PROJECT + f"/source-{index:04}" for index in range(scopes)}
    source_files = {directory + f"/unit-{member:04}.rs"
                    for directory in source_directories for member in range(SOURCE_FILES_PER_DIRECTORY)}
    target_files = {CARGO_TARGET + "/CACHEDIR.TAG", CARGO_TARGET + "/payload.bin"}
    origin = CARGO_PROJECT + "/Cargo.lock"
    project_files = {origin, CARGO_PROJECT + "/Cargo.toml", *target_files, *source_files}
    project_members = {CARGO_PROJECT, CARGO_TARGET, *source_directories, *project_files}
    if ({path for path in evidence if path == CARGO_PROJECT or path.startswith(CARGO_PROJECT + "/")} != project_members
            or any(not stat.S_ISREG(evidence[path][2]) for path in project_files)
            or any(not stat.S_ISDIR(evidence[path][2]) for path in {CARGO_PROJECT, CARGO_TARGET, *source_directories})):
        raise ValueError("Cargo replay requires the exact project, target and unrelated source fixture")
    expected_request = {"action": "dirty", "root_id": before["roots"][0]["id"], "events": [
        {"path": str(fixture / "baseline" / origin), "kind": "file", "recursive": False}]}
    if (record.get("event_paths_verified") is not True or record.get("dirty_request") != expected_request
            or record.get("measured_queue_empty") is not True):
        raise ValueError("Cargo replay must submit one exact typed lock event and drain it before validation")
    if (any(snapshot["stats"]["complete"] is not True for snapshot in snapshots)
            or before["foreground_scan"] != after["foreground_scan"]
            or any(snapshot["candidates"] != before["candidates"] for snapshot in snapshots)):
        raise ValueError("Cargo replay changed full coverage, saved foreground or the positive recommendation")

    def counts(members: set[str]) -> dict[str, int]:
        return {"entries": len(members),
                "files": sum(stat.S_ISREG(evidence[path][2]) for path in members),
                "directories": sum(stat.S_ISDIR(evidence[path][2]) for path in members)}

    full_counts = counts(set(evidence))
    refreshed = project_members if variant == "baseline" else {origin, CARGO_TARGET, *target_files}
    scope_counts = counts(refreshed)
    if (any(before["stats"][key] != value for key, value in full_counts.items())
            or any(after["stats"][key] - before["stats"][key] != value for key, value in scope_counts.items())
            or any(full["stats"][key] != value for key, value in full_counts.items())):
        raise ValueError("Cargo replay must examine the whole project in baseline, origin plus target in candidate, then one full root")
    for key in ("dirty_acknowledgment_seconds", "dirty_acknowledgment_cpu_seconds"):
        value = record[key]
        if isinstance(value, bool) or not isinstance(value, (int, float)) or not math.isfinite(value) or value < 0:
            raise ValueError("Invalid Cargo replay acknowledgment measurement: " + key)
    if (record["dirty_acknowledgment_seconds"] > record["wall_seconds"] + 1e-8
            or record["dirty_acknowledgment_cpu_seconds"] > record["cpu_seconds"] + 1e-8):
        raise ValueError("Cargo replay acknowledgment must lie within the settled measurement")

    states = record["library_states"]
    if set(states) != {"before", "after", "full"}:
        raise ValueError("Cargo replay is missing a read-only index checkpoint")
    indexed = states["before"]["candidates"]
    expected_paths = {str(fixture / "baseline" / path) for path in (ARTIFACT, CARGO_TARGET)}
    if len(indexed) != 2 or {candidate["path"] for candidate in indexed} != expected_paths:
        raise ValueError("Cargo replay must retain both the eligible Node row and measured Cargo diagnostic")
    for key, snapshot in zip(("before", "after", "full"), snapshots):
        state = states[key]
        scoped = state["scope_stats"]
        expected_counts = scope_counts if key == "after" else full_counts
        if (state["candidates"] != indexed or state["foreground_scan"] != snapshot["foreground_scan"]
                or scoped["complete"] is not True or scoped["cancelled"] or scoped["errors"]
                or any(scoped[name] != value for name, value in expected_counts.items())):
            raise ValueError("Cargo replay changed an indexed proof or retained an incomplete or incorrect scope")
    for candidate in indexed:
        relative = str(Path(candidate["path"]).relative_to(fixture / "baseline"))
        info = evidence[relative]
        expected_identity = dict(device=info[0], inode=info[1], mode=info[2], size=info[3],
                                 modified_ns=info[4], changed_ns=info[5])
        files = [value for path, value in evidence.items()
                 if path.startswith(relative + "/") and stat.S_ISREG(value[2])]
        if (candidate["identity"] != expected_identity or candidate["provisional"]
                or candidate["blocked_reason"] is not None or not candidate["fingerprint"] or not candidate["evidence"]
                or candidate["file_count"] != len(files)
                or candidate["logical_bytes"] != sum(value[3] for value in files)
                or candidate["allocated_bytes"] != sum(value[7] for value in files)):
            raise ValueError("Indexed Cargo/Node evidence differs from the independent allocated-file audit")
        if relative == CARGO_TARGET:
            if (candidate["kind"] != "cargo" or candidate["suggestion_eligible"]
                    or candidate["eligible_permanent"] or not 0 < candidate["allocated_bytes"] < 100_000_000):
                raise ValueError("Small Cargo output must remain a completed, size-ineligible diagnostic")
        elif candidate != before["candidates"][0]:
            raise ValueError("Visible positive recommendation differs from its indexed proof")
    record.update(event_count=1, source_directories=scopes, source_files=len(source_files),
                  project_entries=len(project_members), refreshed_entries=scope_counts["entries"],
                  target_entries=1 + len(target_files), full_entries=len(evidence))
    return indexed


def validate(record: dict, fixture: Path, run_id: int, scopes: int, evidence: dict,
             workload: str, variant: str) -> list:
    entries = len(evidence)
    keys = ("before", "after", "full") if workload in (REPLAY, PERIODIC, CARGO_LOCK) else ("before", "after")
    if any(record[key].get("ok") is not True for key in keys):
        raise ValueError("Driver did not receive successful snapshots")
    snapshots = [record[key]["data"] for key in keys]
    before, after = snapshots[:2]
    zero = dict(collected_coins=0, pending_coins=0, fractional_bytes=0, credited_bytes=0)
    for snapshot in snapshots:
        if (snapshot["scanning"] or snapshot["cleaning"] or snapshot["error"]
                or snapshot["wallet"] != zero or snapshot["history"] or snapshot["kept_paths"]
                or snapshot["stats"]["cancelled"] or snapshot["stats"]["errors"]):
            raise ValueError("A snapshot reported failure or changed the synthetic ledger")
        if (snapshot["roots"] != before["roots"] or len(snapshot["roots"]) != 1
                or snapshot["roots"][0]["path"] != str(fixture / "baseline")
                or snapshot["roots"][0]["kind"] != "projects"):
            raise ValueError("The workload changed the authorized fixture root")
    indexed_proof = None
    if workload == REPLAY:
        expected_paths = [str(fixture / "baseline/replay-sentinel")]
        for index in range(scopes):
            name = "missing" if index % 2 else "file"
            expected_paths.append(str(fixture / "baseline" / ARTIFACT / "group-0000" / f"{name}-{index // 2:04}.txt"))
        if record.get("staged") != {"unindexed": True, "cursor": 1000, "pending_paths": expected_paths}:
            raise ValueError("The staged input was not the exact unindexed raw descendant queue")
        if (before["candidates"] or before["stats"]["entries"] != 0
                or before["stats"]["complete"] is not False or after["stats"]["complete"] is not False
                or before.get("foreground_scan") is not None or after.get("foreground_scan") is not None):
            raise ValueError("Replay must start unindexed and must not claim full-root coverage")
        artifact_entries = sum(path == ARTIFACT or path.startswith(ARTIFACT + "/") for path in evidence)
        scope_stats = record["replay_scope_stats"]
        if (record.get("replay_queue_empty") is not True or scope_stats["complete"] is not True
                or scope_stats["cancelled"] or scope_stats["errors"] or scope_stats["entries"] != artifact_entries):
            raise ValueError("Measured replay must finish its scope and durable queue before the untimed full scan")
        expected_passes = 2 if variant == "baseline" else 1
        if after["stats"]["entries"] != expected_passes * artifact_entries:
            raise ValueError(f"Expected {expected_passes} artifact traversals ({expected_passes * artifact_entries} entries); observed {after['stats']['entries']}")
        record["artifact_entries"] = artifact_entries
        record["artifact_passes"] = after["stats"]["entries"] // artifact_entries
        record["staged_raw_scopes"] = scopes
        full = snapshots[2]
        if (full["stats"]["complete"] is not True or full["stats"]["entries"] != entries
                or full["candidates"] != after["candidates"]):
            raise ValueError("The untimed full scan changed the recommendation or lost whole-root coverage")
        foreground = full.get("foreground_scan")
    elif workload == PERIODIC:
        if any(snapshot["stats"]["complete"] is not True for snapshot in snapshots):
            raise ValueError("Periodic work lost complete root coverage")
        validate_periodic(record, fixture, evidence, snapshots)
        foreground = snapshots[2].get("foreground_scan")
        original = before.get("foreground_scan")
        if (not isinstance(original, dict) or original.get("active") is not False
                or original["stats"]["complete"] is not True or original["stats"]["cancelled"]
                or original["stats"]["errors"] or original["stats"]["entries"] != entries):
            raise ValueError("Periodic work did not preserve an initially completed foreground scan")
    elif workload == ARTIFACT_BATCH:
        validate_artifact_batch(record, fixture, scopes, evidence, snapshots)
        foreground = before.get("foreground_scan")
    elif workload == CARGO_LOCK:
        indexed_proof = validate_cargo_lock(record, fixture, scopes, evidence, snapshots, variant)
        foreground = snapshots[2].get("foreground_scan")
        original = before.get("foreground_scan")
        if (not isinstance(original, dict) or original.get("active") is not False
                or original["stats"]["complete"] is not True or original["stats"]["cancelled"]
                or original["stats"]["errors"] or original["stats"]["entries"] != entries):
            raise ValueError("Cargo replay did not preserve an initially completed foreground scan")
    else:
        if any(snapshot["stats"]["complete"] is not True for snapshot in snapshots):
            raise ValueError("A snapshot lost full-root coverage")
        if before["stats"]["entries"] != entries or after["stats"]["entries"] != entries + scopes:
            raise ValueError("The engine did not examine the expected existing and missing scopes")
        if before["foreground_scan"] != after["foreground_scan"]:
            raise ValueError("Maintenance changed the completed foreground scan")
        if before["candidates"] != after["candidates"]:
            raise ValueError("Maintenance changed the unrelated positive recommendation")
        foreground = before.get("foreground_scan")
    if (not isinstance(foreground, dict) or foreground.get("active") is not False
            or foreground["stats"]["complete"] is not True or foreground["stats"]["cancelled"]
            or foreground["stats"]["errors"] or foreground["stats"]["entries"] != entries):
        raise ValueError("The validation scan did not complete its finite foreground coverage")
    if len(after["candidates"]) != 1:
        raise ValueError("Expected exactly one useful recommendation")
    candidate = after["candidates"][0]
    expected = fixture / "baseline" / ARTIFACT
    file_count = sum(path.startswith(ARTIFACT + "/") and stat.S_ISREG(value[2]) for path, value in evidence.items())
    if (candidate["path"] != str(expected) or candidate["suggestion_eligible"] is not True
            or candidate["provisional"] or candidate["blocked_reason"] is not None
            or not candidate["eligible_permanent"] or candidate["allocated_bytes"] < 100_000_000
            or candidate["file_count"] != file_count or not candidate["fingerprint"] or not candidate["evidence"]):
        raise ValueError("Missing expected eligible artifact")
    if workload in (ARTIFACT_BATCH, CARGO_LOCK):
        files = [value for path, value in evidence.items()
                 if path.startswith(ARTIFACT + "/") and stat.S_ISREG(value[2])]
        if (candidate["logical_bytes"] != sum(value[3] for value in files)
                or candidate["allocated_bytes"] != sum(value[7] for value in files)):
            raise ValueError("The candidate's logical and allocated bytes differ from the independent file audit")
    database = fixture / f"state-{run_id}.sqlite"
    with sqlite3.connect(database.as_uri() + "?mode=ro", uri=True) as connection:
        connection.execute("PRAGMA query_only=ON")
        for table in ("pending_scopes", "active_scopes", "refreshes", "refresh_seen", "incomplete_roots", "operations", "earnings", "windows", "allocations"):
            if connection.execute("SELECT count(*) FROM " + table).fetchone()[0] != 0:
                raise ValueError(f"Unexpected durable work or ledger rows in {table}")
        if connection.execute("SELECT cursor FROM event_cursor").fetchone() != (1017 if workload == PERIODIC else 1000,):
            raise ValueError("Event receipt cursor was not persisted")
        if workload == ARTIFACT_BATCH:
            rows = connection.execute("SELECT json FROM scans").fetchall()
            if len(rows) != 1:
                raise ValueError("Artifact batch did not retain exactly one root's completed scope statistics")
            scoped = json.loads(rows[0][0])
            if (scoped["complete"] is not True or scoped["cancelled"] or scoped["errors"]
                    or scoped["entries"] != record["artifact_entries"]
                    or scoped["files"] != file_count or scoped["directories"] != 1):
                raise ValueError("The final stored scope was not one complete artifact traversal")
            saved = connection.execute("SELECT summary_json FROM foreground_state WHERE id=1").fetchone()
            if saved is None or saved[0] is None or json.loads(saved[0]) != foreground:
                raise ValueError("Artifact maintenance changed the durable completed foreground summary")
            if (connection.execute("SELECT collected,remainder,credited FROM wallet WHERE id=1").fetchall() != [(0, 0, 0)]
                    or connection.execute("SELECT count(*) FROM kept").fetchone() != (0,)
                    or connection.execute("SELECT count(*) FROM candidate_tombstones").fetchone() != (0,)):
                raise ValueError("Artifact maintenance changed the synthetic wallet or retained review state")
        if workload == CARGO_LOCK:
            indexed = [json.loads(row[0]) for row in connection.execute("SELECT json FROM candidates ORDER BY path")]
            scans = [json.loads(row[0]) for row in connection.execute("SELECT json FROM scans")]
            saved = connection.execute("SELECT summary_json FROM foreground_state WHERE id=1").fetchone()
            final_state = record["library_states"]["full"]
            if (indexed != indexed_proof or scans != [final_state["scope_stats"]]
                    or saved is None or saved[0] is None or json.loads(saved[0]) != foreground
                    or connection.execute("SELECT collected,remainder,credited FROM wallet WHERE id=1").fetchall() != [(0, 0, 0)]
                    or connection.execute("SELECT count(*) FROM kept").fetchone() != (0,)
                    or connection.execute("SELECT count(*) FROM candidate_tombstones").fetchone() != (0,)):
                raise ValueError("Cargo replay's final durable state differs from its captured validation proof")
        if connection.execute("PRAGMA integrity_check").fetchone() != ("ok",):
            raise ValueError("Disposable SQLite library failed its integrity check")
    if record["scopes"] != scopes or record["workload"] != workload:
        raise ValueError("Unexpected workload or scope count")
    if (any(not math.isfinite(record[key]) or record[key] < 0 for key in ("wall_seconds", "cpu_seconds"))
            or record["lifetime_peak_rss_bytes"] <= 0):
        raise ValueError("Invalid timing or memory measurements")
    proof = [candidate[key] for key in ("path", "identity", "fingerprint", "evidence", "allocated_bytes", "file_count")]
    if workload == ARTIFACT_BATCH:
        proof.append(candidate["logical_bytes"])
    if indexed_proof is not None:
        proof.append(indexed_proof)
    return proof


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline-archive", type=Path, required=True)
    parser.add_argument("--candidate-archive", type=Path, required=True)
    parser.add_argument("--warm-runs", type=int, default=5)
    parser.add_argument("--workload", choices=("mixed", REPLAY, PERIODIC, ARTIFACT_BATCH, CARGO_LOCK), default="mixed")
    parser.add_argument("--scopes", type=int, help="Defaults: 256 mixed events, 32 replay paths, 512 artifact events, or 512 unrelated Cargo source directories (16 files each); periodic is fixed at 16. Artifact/Cargo modes allow 1..512; other modes require an even count.")
    parser.add_argument("--artifact-files", type=int, default=4096, help="Small files inside the replay artifact (default: 4096)")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if platform.system() != "Darwin" or not 1 <= args.warm_runs <= 100:
        parser.error("Use macOS and 1..100 warm runs")
    replay = args.workload == REPLAY
    periodic = args.workload == PERIODIC
    artifact_batch = args.workload == ARTIFACT_BATCH
    cargo_lock = args.workload == CARGO_LOCK
    args.scopes = args.scopes if args.scopes is not None else (16 if periodic else 32 if replay else 512 if artifact_batch or cargo_lock else 256)
    maximum_scopes = 64 if replay else 512
    if artifact_batch and not 1 <= args.scopes <= 512:
        parser.error("Use 1..512 distinct artifact file events")
    if cargo_lock and not 1 <= args.scopes <= 512:
        parser.error("Use 1..512 unrelated Cargo source directories")
    if not artifact_batch and not cargo_lock and (not 2 <= args.scopes <= maximum_scopes or args.scopes % 2):
        parser.error(f"Use an even scope count from 2 to {maximum_scopes}")
    if periodic and args.scopes != 16:
        parser.error("Periodic background work uses exactly sixteen events")
    if not 1 <= args.artifact_files <= 16_384 or (replay and args.artifact_files < args.scopes // 2):
        parser.error("Use 1..16384 artifact files, at least half the replay scope count")
    output = args.output.resolve()
    if (REPO / "benchmarks/local").resolve() not in output.parents:
        parser.error("Output must be a new directory below benchmarks/local")
    output.mkdir(parents=True, exist_ok=False)
    if replay:
        method = "Real engine FFI: stage raw unkeep descendant paths behind one ordinary missing-scope debounce, cancel and close, then prove the exact pending queue and empty index with read-only SQLite. No initial scan. Each invocation uses a new library and the same unchanged scanned tree. Time only Resume through settled snapshot delivery after reopening; wall/process CPU include worker startup, journal work and 5 ms snapshot polling. Staging, cursor receipt, cancellation, reopening and the final full-coverage validation scan are excluded. RSS is lifetime peak through the measured endpoint, including staging. Baseline must examine the artifact twice and candidate once; equivalent final proofs and an untimed full scan are required. Setup races reject the comparison without automatic retries. This measures queued refresh normalization, not full-scan throughput or the reported Home slowdown."
    elif periodic:
        method = "Real engine FFI, sixteen identical typed recursive directory events at absolute 400 ms deadlines (t=0 through 6 s), each followed by durable cursor receipt. New library per invocation; unchanged two-entry scope and unrelated positive artifact. Process CPU is measured over a fixed eight-second window including the build's configured background debounce and one endpoint snapshot; no intermediate snapshot polling. Each request and receipt must finish within 50 ms of its deadline, and the endpoint within 50 ms of eight seconds. Raw entry/file/directory deltas count completed scope traversals, not workers or transactions; no pass-count reduction or speedup is assumed. Exact empty queues and frozen foreground/proof are required before a separate Scan probe. That probe submits an event, waits until +50 ms, proves the exact pending scope and no active work through read-only SQLite, then submits Scan before +100 ms. Scan acknowledgment and completed-foreground delivery are reported separately; completion includes full-root traversal and 5 ms snapshot polling, so it is an upper bound on worker wake-up, not pure scheduling latency. Setup, initial scanning, queue proofs and the probe are excluded from periodic CPU. RSS is process lifetime through the periodic endpoint, including setup and initial scanning but excluding the subsequent queue proof and Scan probe. Timing misses or invalid state abort without retries. Fixed-window wall time is not throughput, and this synthetic FFI workload does not measure native FSEvents or SwiftUI CPU."
    elif artifact_batch:
        method = "Real engine FFI, one typed file-event batch for distinct existing event-NNNN.txt files inside one unchanged aged eligible artifact with a 100 MiB payload. Each invocation uses a new library and the same independently audited tree. Setup, initial full discovery and input-path verification are excluded. Dirty acknowledgment wall/process CPU include only the dirty request and response disposal, before cursor receipt or snapshot polling; this includes event classification, durable journal work and worker launch, not just SQLite. Total settled wall/process CPU include that request, cursor receipt, each build's configured background debounce and 5 ms snapshot polling. Both variants must examine the same artifact exactly once and preserve the entire candidate, full-root coverage and saved foreground. Read-only SQLite checks require the final artifact scope, drained queue, cursor receipt and unchanged zero ledger; the filesystem audit covers identities, timestamps and contents. RSS is process lifetime through the settled endpoint, including setup and initial discovery. This isolates duplicate event ingestion, not a reduction in traversal or native FSEvents/SwiftUI work."
    elif cargo_lock:
        method = "Real engine FFI, one typed nonrecursive file event for the existing exact Cargo.lock path in an unchanged aged Cargo project. This is event replay, not a lockfile rewrite. The project contains a standard target marker and small payload, plus the requested number of unrelated source directories with sixteen ordinary files each. A separate 100 MiB Node artifact remains eligible. Each invocation uses a fresh library; both builds use the same independently audited tree and an empty synthetic CARGO_HOME with process target-directory overrides unset. Setup, initial full discovery, initial index capture and event-path verification are excluded. Dirty acknowledgment wall/process CPU end after request/response disposal, before cursor receipt. Total settled wall/process CPU include that request, durable cursor receipt, the build's configured background debounce and 5 ms snapshot polling. Baseline must traverse the whole Cargo project; candidate must traverse only Cargo.lock and the immediate target footprint. Read-only SQLite captures prove the exact completed scope, unchanged full index including the size-ineligible Cargo diagnostic, saved foreground, drained queue and zero ledger before an untimed explicit full Scan verifies equivalent whole-root coverage. Candidate identities, fingerprints, evidence and logical/allocated bytes must agree across phases and builds; independent before/after audits cover file contents, allocation and metadata. RSS is process lifetime through the measured endpoint, including setup and initial discovery, excluding endpoint index capture and the validation Scan. This measures a Cargo.lock refresh dependency, not native FSEvents/SwiftUI CPU or full-scan throughput."
        method += " Entry deltas count scanner coverage, excluding the candidate's shallow exact-name guard enumeration; guard time and CPU remain inside the measurement."
    else:
        method = "Real engine FFI, one typed batch of equal existing and absent directory scopes; each invocation uses a new library and the same unchanged scanned tree. Setup and initial discovery excluded. Monotonic wall/process CPU include each build's configured background debounce, journal work, cursor receipt and 5 ms snapshot polling. RSS is process lifetime including setup."
    report = {"hardware": benchmark.hardware(), "verified": False, "runs": [], "workload": args.workload,
        "harness_sha256": digest(Path(__file__)), "driver_source_sha256": digest(REPO / "scripts/event-benchmark.c"),
        "method": method + " No profiler or transaction hooks are attached. First run is not a cold-cache measurement."}
    summary = output / "summary.json"
    try:
        report["builds"] = {name: compile_driver(getattr(args, name + "_archive").resolve(strict=True), output / (name + "-driver"))
                            for name in ("baseline", "candidate")}
        fixture, evidence = make_fixture(args.scopes, args.workload, args.artifact_files)
        report.update(fixture=str(fixture), audit_before=evidence)
        environment = None
        if cargo_lock:
            environment = os.environ.copy()
            environment["CARGO_HOME"] = str(fixture / "cargo-home")
            for name in ("CARGO_TARGET_DIR", "CARGO_BUILD_TARGET_DIR"):
                environment.pop(name, None)
            report["cargo_environment"] = {"CARGO_HOME": environment["CARGO_HOME"],
                                           "unset": ["CARGO_TARGET_DIR", "CARGO_BUILD_TARGET_DIR"]}
        reference = None
        run_id = 0
        for number in range(args.warm_runs + 1):
            for name in (("baseline", "candidate") if number % 2 == 0 else ("candidate", "baseline")):
                run_id += 1
                driver = output / (name + "-driver")
                if digest(driver) != report["builds"][name]["driver_sha256"]:
                    raise ValueError("Driver changed during comparison")
                result = subprocess.run([str(driver), str(fixture), str(args.scopes), str(run_id), args.workload],
                                        capture_output=True, text=True, timeout=90, env=environment)
                (output / f"{run_id:03}-{name}.stdout.json").write_text(result.stdout)
                (output / f"{run_id:03}-{name}.stderr.log").write_text(result.stderr)
                result.check_returncode()
                record = json.loads(result.stdout)
                proof = validate(record, fixture, run_id, args.scopes, evidence, args.workload, name)
                reference = proof if reference is None else reference
                if proof != reference:
                    raise ValueError("Equivalent event workloads returned different recommendation proofs")
                record.pop("before"); record.pop("after")
                record.pop("full", None); record.pop("staged", None)
                record.pop("replay_scope_stats", None)
                if artifact_batch or cargo_lock:
                    record.pop("dirty_request")
                if cargo_lock:
                    record.pop("library_states")
                if periodic:
                    record["probe"].pop("gate")
                record.update(variant=name, phase="warm" if number else "first", run_id=run_id, verified=True)
                report["runs"].append(record)
                summary.write_text(json.dumps(report, indent=2) + "\n")
        report["audit_after"] = audit(fixture / "baseline", include_allocated=artifact_batch or cargo_lock)
        if report["audit_after"] != evidence:
            raise ValueError("The scanned fixture changed")
        if cargo_lock:
            cargo_home = fixture / "cargo-home"
            info = cargo_home.lstat()
            if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.geteuid() or any(cargo_home.iterdir()):
                raise ValueError("The synthetic Cargo home changed or contains configuration")
        report["summary"] = {}
        for name in ("baseline", "candidate"):
            rows = [r for r in report["runs"] if r["variant"] == name and r["phase"] == "warm"]
            report["summary"][name] = {"samples": len(rows),
                "median_wall_ms": statistics.median(r["wall_seconds"] * 1000 for r in rows),
                "p95_wall_ms": benchmark.percentile95([r["wall_seconds"] * 1000 for r in rows]),
                "median_cpu_ms": statistics.median(r["cpu_seconds"] * 1000 for r in rows),
                "maximum_peak_rss_bytes": max(r["lifetime_peak_rss_bytes"] for r in rows)}
            if replay:
                report["summary"][name]["artifact_passes"] = sorted({r["artifact_passes"] for r in rows})
                report["summary"][name]["artifact_entries"] = rows[0]["artifact_entries"]
            if artifact_batch:
                report["summary"][name].update(
                    artifact_passes=sorted({r["artifact_passes"] for r in rows}),
                    artifact_entries=rows[0]["artifact_entries"],
                    event_count=args.scopes,
                    median_dirty_acknowledgment_ms=statistics.median(r["dirty_acknowledgment_seconds"] * 1000 for r in rows),
                    p95_dirty_acknowledgment_ms=benchmark.percentile95([r["dirty_acknowledgment_seconds"] * 1000 for r in rows]),
                    median_dirty_acknowledgment_cpu_ms=statistics.median(r["dirty_acknowledgment_cpu_seconds"] * 1000 for r in rows),
                    p95_dirty_acknowledgment_cpu_ms=benchmark.percentile95([r["dirty_acknowledgment_cpu_seconds"] * 1000 for r in rows]))
            if cargo_lock:
                report["summary"][name].update(
                    event_count=1,
                    source_directories=args.scopes,
                    source_files=rows[0]["source_files"],
                    project_entries=rows[0]["project_entries"],
                    refreshed_entries=sorted({r["refreshed_entries"] for r in rows}),
                    target_entries=rows[0]["target_entries"],
                    full_entries=rows[0]["full_entries"],
                    median_dirty_acknowledgment_ms=statistics.median(r["dirty_acknowledgment_seconds"] * 1000 for r in rows),
                    median_dirty_acknowledgment_cpu_ms=statistics.median(r["dirty_acknowledgment_cpu_seconds"] * 1000 for r in rows))
            if periodic:
                report["summary"][name].update(
                    window_seconds=8,
                    scope_entries=2,
                    completed_scope_passes=sorted({r["completed_scope_passes"] for r in rows}),
                    median_completed_scope_passes=statistics.median(r["completed_scope_passes"] for r in rows),
                    median_scan_acknowledgment_ms=statistics.median(r["probe"]["scan_acknowledgment_seconds"] * 1000 for r in rows),
                    median_scan_completion_ms=statistics.median(r["probe"]["scan_completion_seconds"] * 1000 for r in rows),
                    p95_scan_completion_ms=benchmark.percentile95([r["probe"]["scan_completion_seconds"] * 1000 for r in rows]))
        report["verified"] = True
        print(json.dumps(report["summary"], indent=2))
    except Exception as error:
        report["error"] = str(error)
        raise
    finally:
        summary.write_text(json.dumps(report, indent=2) + "\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
