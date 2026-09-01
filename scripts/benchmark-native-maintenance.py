#!/usr/bin/env python3
"""Measure one fixed native watcher/engine/UI window on newly created disposable files.

Run baseline and candidate apps separately with the same harness. This is a
finite background workload, not a full-scan or idle-CPU benchmark. No cleanup
operation runs and every fixture is retained.
"""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import math
import os
from pathlib import Path
import shutil
import signal
import sqlite3
import stat
import subprocess
import time
import uuid

import benchmark

_spec = importlib.util.spec_from_file_location("chippytea_native_cpu", Path(__file__).with_name("benchmark-native-cpu.py"))
if _spec is None or _spec.loader is None:
    raise RuntimeError("The shared native benchmark helpers could not be loaded")
energy = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(energy)

PROTOCOL = 2
PULSES = 16
INTERVAL_NS = 400_000_000
FIRST_NS = 250_000_000
LATE_NS = 50_000_000
PAYLOAD_BYTES = 100 * 1024**2
CHANGED = "Changed/Cargo.toml"
TAG = b"Signature: 8a477f597d28d172789f06886806bc55\n"
LEDGER_TABLES = ("wallet", "earnings", "operations", "windows", "allocations")


def digest(path: Path) -> str:
    value = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024**2), b""):
            value.update(chunk)
    return value.hexdigest()


def manifest(version: int) -> bytes:
    return f'[package]\nname="changed"\nversion="0.0.{version}"\n'.encode()


def write_json(path: Path, value: dict) -> None:
    with path.open("x") as stream:
        json.dump(value, stream, indent=2)
        stream.write("\n")


def make_fixture() -> tuple[Path, dict]:
    temporary = Path("/private/tmp")
    if temporary.resolve(strict=True) != temporary or not temporary.is_dir():
        raise RuntimeError("The native fixture requires physical /private/tmp")
    if shutil.disk_usage(temporary).free < 3 * 1024**3 + PAYLOAD_BYTES + 8 * 1024**2:
        raise RuntimeError("Fixture creation requires its payload plus a 3 GiB free-space reserve")
    fixture = temporary / f"chippytea-native-maintenance-{uuid.uuid4()}"
    fixture.mkdir(mode=0o700)
    root = fixture / "baseline"
    positive = root / "Positive"
    changed = root / "Changed"
    (positive / "target").mkdir(parents=True)
    (changed / "target").mkdir(parents=True)
    contents = {
        "Positive/Cargo.toml": b'[package]\nname="positive"\nversion="0.1.0"\n',
        "Positive/preserve.txt": b"Preserve this sibling source.\n",
        "Positive/target/CACHEDIR.TAG": TAG,
        "Changed/target/CACHEDIR.TAG": TAG,
        CHANGED: manifest(100),
    }
    for relative, data in contents.items():
        with (root / relative).open("xb") as stream:
            stream.write(data)
    with (positive / "target/payload").open("xb") as stream:
        for _ in range(100):
            stream.write(b"9" * 1024**2)
        stream.flush()
        os.fsync(stream.fileno())
    old = time.time_ns() - 9 * 86_400 * 1_000_000_000
    for directory, _, filenames in os.walk(root, topdown=False, followlinks=False):
        for name in filenames:
            path = Path(directory) / name
            if path != root / CHANGED:
                os.utime(path, ns=(old, old), follow_symlinks=False)
        os.utime(directory, ns=(old, old), follow_symlinks=False)
    expected = {"files": 6, "directories_including_root": 5, "entries_including_root": 11,
                "logical_regular_file_bytes": PAYLOAD_BYTES + sum(map(len, contents.values())),
                "symlinks": 0, "special_files": 0}
    marker = {"magic": benchmark.MAGIC, "status": "complete", "baseline_relative_path": "baseline",
              "baseline": expected, "workload": "native-periodic-ownership-v1",
              "expected_eligible_relative_paths": ["Positive/target"],
              "suggestions_coverage": {"entries": 10, "files": 5, "directories": 5},
              "coverage_note": "Changed/target is recognized but pruned because its manifest is fresh; its tag is intentionally outside Suggestions traversal."}
    write_json(fixture / benchmark.MARKER, marker)
    return fixture, marker


def identity(info: os.stat_result) -> dict:
    return {"device": info.st_dev, "inode": info.st_ino, "mode": info.st_mode, "size": info.st_size,
            "modifiedNs": info.st_mtime_ns, "changedNs": info.st_ctime_ns,
            "owner": info.st_uid, "links": info.st_nlink, "flags": getattr(info, "st_flags", 0)}


def file_audit(root: Path) -> dict:
    result = {}
    for directory, directories, files in os.walk(root, followlinks=False):
        for path in [Path(directory), *(Path(directory) / name for name in files)]:
            info = path.lstat()
            if (info.st_uid != os.geteuid() or getattr(info, "st_flags", 0) & 0x40000000
                    or not (stat.S_ISDIR(info.st_mode) or stat.S_ISREG(info.st_mode))
                    or (stat.S_ISREG(info.st_mode) and info.st_nlink != 1)):
                raise RuntimeError("Fixture identity/type changed")
            value = identity(info)
            if stat.S_ISREG(info.st_mode):
                value["sha256"] = digest(path)
            result[str(path.relative_to(root))] = value
        if any((Path(directory) / name).is_symlink() for name in directories):
            raise RuntimeError("Fixture acquired a directory symlink")
    return result


def journal(database: Path, root: Path, expected_modified_ns: int) -> dict:
    os.close(energy.physical_file(database))
    with sqlite3.connect(database.as_uri() + "?mode=ro", uri=True) as connection:
        connection.execute("BEGIN")
        pending = {table: connection.execute(f"SELECT count(*) FROM {table}").fetchone()[0]
                   for table in ("pending_scopes", "active_scopes", "refreshes", "refresh_seen", "incomplete_roots")}
        rows = connection.execute("SELECT json FROM candidates WHERE path=?", (str(root / "Changed/target"),)).fetchall()
        if len(rows) != 1:
            raise RuntimeError("Missing exact tiny artifact diagnostic")
        diagnostic = json.loads(rows[0][0])
        if (any(pending.values()) or diagnostic.get("modified_ns") != expected_modified_ns
                or diagnostic.get("suggestion_eligible") is not False or diagnostic.get("eligible_permanent") is not False
                or diagnostic.get("provisional") is not False or not diagnostic.get("blocked_reason")):
            raise RuntimeError("The final manifest was not indexed with a drained, conservative journal")
        scans = connection.execute("SELECT json FROM scans").fetchall()
        if len(scans) != 1:
            raise RuntimeError("Expected one disposable root's stored scope statistics")
        ledger = {table: connection.execute(f"SELECT * FROM {table} ORDER BY rowid").fetchall() for table in LEDGER_TABLES}
        if ledger["windows"] or ledger["allocations"]:
            raise RuntimeError("The no-cleanup fixture must have no accounting windows or space-credit reservations")
        return {"queues": pending, "diagnostic": diagnostic, "stored_scope_stats": json.loads(scans[0][0]),
                "cursor": connection.execute("SELECT cursor FROM event_cursor WHERE id=1").fetchone()[0],
                "ledger": ledger}


def wait_output(process: subprocess.Popen, output: Path, name: str, seconds: float) -> dict:
    deadline = time.monotonic() + seconds
    while not (output / name).exists():
        if name != "final.json" and (output / "final.json").exists():
            raise RuntimeError(f"Native benchmark rejected the run: {energy.bounded_json(output / 'final.json')}")
        if process.poll() is not None or time.monotonic() >= deadline:
            raise RuntimeError("Native maintenance protocol timed out; inspect process.log")
        time.sleep(0.005)
    return energy.bounded_json(output / name)


def validate_state(record: dict, process: subprocess.Popen, root: Path, reduced: bool, phase: str) -> None:
    if (record.get("maintenance_protocol") != PROTOCOL or record.get("pid") != process.pid
            or record.get("phase") != phase or record.get("root_path") != str(root)
            or record.get("destination") != "Find space" or record.get("visible") is not True
            or record.get("panel_visible") is not True or record.get("occluded") is not False
            or record.get("reduce_motion") is not reduced or record.get("effective_reduce_motion") is not reduced):
        raise RuntimeError(f"Native window/protocol state failed: {record}")


def validate_snapshot(snapshot: dict, before: dict | None, root: Path, audit: dict) -> None:
    stats = snapshot.get("stats", {})
    foreground = snapshot.get("foregroundScan", {})
    if (snapshot.get("scanning") is not False or snapshot.get("cleaning") is not False or snapshot.get("error") is not None
            or stats.get("complete") is not True or stats.get("cancelled") is not False or stats.get("errors") != 0
            or foreground.get("active") is not False or foreground.get("stats", {}).get("complete") is not True
            or foreground.get("stats", {}).get("cancelled") is not False or foreground.get("stats", {}).get("errors") != 0):
        raise RuntimeError("Native snapshot lacks complete background and foreground coverage")
    candidates = snapshot.get("candidates", [])
    if (len(candidates) != 1 or candidates[0].get("path") != str(root / "Positive/target")
            or candidates[0].get("suggestionEligible") is not True or candidates[0].get("eligiblePermanent") is not True
            or candidates[0].get("blockedReason") is not None or not candidates[0].get("fingerprint") or not candidates[0].get("evidence")
            or candidates[0].get("allocatedBytes", 0) < 100_000_000
            or candidates[0].get("identity") != {key: audit["Positive/target"][key] for key in
                                               ("device", "inode", "mode", "size", "modifiedNs", "changedNs")}):
        raise RuntimeError("The visible positive candidate or its exact identity was lost")
    roots = snapshot.get("roots", [])
    if len(roots) != 1 or roots[0].get("path") != str(root) or roots[0].get("kind") != "folder":
        raise RuntimeError("Unexpected authorized root")
    if (snapshot.get("wallet") != {"collectedCoins": 0, "pendingCoins": 0, "fractionalBytes": 50_000_000, "creditedBytes": 50_000_000}
            or snapshot.get("history") != [] or snapshot.get("keptPaths") != []):
        raise RuntimeError("The synthetic wallet/history changed")
    if before is not None and any(snapshot.get(key) != before.get(key) for key in
                                  ("roots", "candidates", "wallet", "history", "keptPaths", "foregroundScan")):
        raise RuntimeError("Candidate proof, ledger or completed foreground changed")


def write_pulses(path: Path, before: dict, start_ns: int, process: subprocess.Popen) -> list[dict]:
    records = []
    previous_modified = before["modifiedNs"]
    for index in range(PULSES):
        deadline = start_ns + FIRST_NS + index * INTERVAL_NS
        remaining = deadline - time.clock_gettime_ns(time.CLOCK_MONOTONIC_RAW)
        if remaining > 0:
            time.sleep(remaining / 1_000_000_000)
        submitted = time.clock_gettime_ns(time.CLOCK_MONOTONIC_RAW)
        if process.poll() is not None or not deadline <= submitted <= deadline + LATE_NS:
            raise RuntimeError("A manifest write missed its absolute deadline; this run is invalid")
        # Each pulse closes before waiting for the next deadline. Retain the
        # inode, never truncate or replace it, and check before mutation as well.
        descriptor = os.open(path, os.O_RDWR | os.O_CLOEXEC | os.O_NONBLOCK | getattr(os, "O_NOFOLLOW_ANY", 0x20000000))
        try:
            info = identity(os.fstat(descriptor))
            if any(info[key] != before[key] for key in ("device", "inode", "mode", "size", "owner", "links", "flags")):
                raise RuntimeError("The manifest changed before a pulse")
            data = manifest(101 + index)
            if len(data) != before["size"] or os.pwrite(descriptor, data, 0) != len(data):
                raise RuntimeError("The same-inode write was not exact")
            os.fsync(descriptor)
            info = identity(os.fstat(descriptor))
        finally:
            os.close(descriptor)
        acknowledged = time.clock_gettime_ns(time.CLOCK_MONOTONIC_RAW)
        if (acknowledged > deadline + LATE_NS or info["modifiedNs"] <= previous_modified
                or any(info[key] != before[key] for key in ("device", "inode", "mode", "size", "owner", "links", "flags"))):
            raise RuntimeError("Manifest identity, fresh timestamp or write acknowledgment deadline failed")
        records.append({"number": index + 1, "deadline_monotonic_raw_ns": deadline,
                        "submitted_monotonic_raw_ns": submitted, "acknowledged_monotonic_raw_ns": acknowledged,
                        "modified_ns": info["modifiedNs"]})
        previous_modified = info["modifiedNs"]
    return records


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--app", type=Path, required=True)
    parser.add_argument("--core-archive", type=Path, required=True, help="Exact linked archive, recorded and checked for mutation; linkage provenance remains the build record")
    parser.add_argument("--output", type=Path, required=True, help="New directory under benchmarks/local")
    parser.add_argument("--reduce-motion", action="store_true")
    args = parser.parse_args()
    if not hasattr(time, "CLOCK_MONOTONIC_RAW"):
        parser.error("This macOS workload requires the shared CLOCK_MONOTONIC_RAW clock")
    output = args.output.resolve()
    if benchmark.REPO / "benchmarks/local" not in output.parents:
        parser.error("Use a new output directory under benchmarks/local")
    output.mkdir(parents=True, exist_ok=False)
    binary = args.app.resolve(strict=True) / "Contents/MacOS/chippytea"
    archive = args.core_archive.resolve(strict=True)
    hashes = {"app": digest(binary), "core_archive": digest(archive), "runner": digest(Path(__file__)),
              "shared_helpers": digest(Path(energy.__file__))}
    fixture, marker = make_fixture()
    root, state = fixture / "baseline", fixture / "state"
    state.mkdir(mode=0o700)
    write_json(fixture / ".chippytea-energy-state.json", {"magic": "chippytea-synthetic-energy-state-v1", "coins": 0})
    write_json(output / "fixture.json", {"path": str(fixture), "marker": marker})
    audit_before = file_audit(root)
    coverage_before = benchmark.verify_fixture(root, marker["baseline"])
    if not coverage_before["matches_marker"]:
        raise RuntimeError("The newly generated fixture failed its independent audit")
    environment = dict(os.environ)
    environment.update(CHIPPYTEA_DATA_DIR=str(state), CHIPPYTEA_SCREENSHOT_ROOT=str(root),
                       CHIPPYTEA_SCREENSHOT_STATE="discover", CHIPPYTEA_SCREENSHOT=str(output / "window.png"),
                       CHIPPYTEA_ENERGY_OUTPUT=str(output / "ready.json"))
    preferences = ["-reduceMotion", "YES" if args.reduce_motion else "NO"]
    with (output / "stage.log").open("x") as log:
        subprocess.run([str(binary), "--screenshot", *preferences], env=environment,
                       stdout=log, stderr=subprocess.STDOUT, timeout=90, check=True)
    energy.validate_state(state, root, None)
    database = state / "library.sqlite"
    # Only this fresh marked library is seeded. Preserve the existing isolation
    # guard's synthetic half-coin remainder; no cleanup or earning is invented.
    with sqlite3.connect(database) as connection:
        connection.execute("UPDATE wallet SET remainder=50000000,credited=50000000 WHERE id=1")
    energy.validate_state(state, root, 0)
    command = [str(binary), "--maintenance-benchmark", *preferences]
    report = None
    with (output / "process.log").open("x") as log:
        process = subprocess.Popen(command, env=environment, stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
        try:
            ready = wait_output(process, output, "ready.json", 65)
            validate_state(ready, process, root, args.reduce_motion, "ready")
            if (ready.get("window_seconds"), ready.get("first_write_offset_ms"), ready.get("write_interval_ms"), ready.get("writes")) != (8, 250, 400, 16):
                raise RuntimeError("The app advertised a different workload")
            before = ready["snapshot"]
            validate_snapshot(before, None, root, audit_before)
            if any(before["stats"].get(key) != value for key, value in marker["suggestions_coverage"].items()):
                raise RuntimeError("Setup did not finish the fixture's exact Suggestions coverage")
            before_journal = journal(database, root, audit_before[CHANGED]["modifiedNs"])
            time.sleep(3)  # Settle the ordinary entrance before native CPU capture.
            os.kill(process.pid, signal.SIGUSR1)  # Only the advertised protocol installs this handler.
            started = wait_output(process, output, "started.json", 2)
            validate_state(started, process, root, args.reduce_motion, "measuring")
            start_ns = started.get("start_monotonic_raw_ns")
            if type(start_ns) is not int or start_ns <= 0:
                raise RuntimeError("Missing shared monotonic start")
            writes = write_pulses(root / CHANGED, audit_before[CHANGED], start_ns, process)
            write_json(output / "writes.json", {"clock": "CLOCK_MONOTONIC_RAW", "writes": writes})
            endpoint = wait_output(process, output, "endpoint.json", 4)
            validate_state(endpoint, process, root, args.reduce_motion, "endpoint")
            endpoint_journal = journal(database, root, writes[-1]["modified_ns"])
            final = wait_output(process, output, "final.json", 4)
            validate_state(final, process, root, args.reduce_motion, "complete")
            after_journal = journal(database, root, writes[-1]["modified_ns"])
            if (final.get("valid") is not True or final.get("fresh_snapshot_matches_frozen") is not True
                    or final.get("quiet_tail_seconds") != 1.2 or final.get("quiet_tail_unchanged") is not True
                    or endpoint_journal != after_journal or after_journal["cursor"] <= before_journal["cursor"]
                    or after_journal["ledger"] != before_journal["ledger"]
                    or any(final.get(key) != value for key, value in endpoint.items() if key != "phase")):
                raise RuntimeError("The frozen endpoint or its independent quiet-tail proof changed")
            after = final["snapshot"]
            validate_snapshot(after, before, root, audit_before)
            for key in ("wall_seconds", "user_cpu_seconds", "system_cpu_seconds", "cpu_seconds"):
                value = final.get(key)
                if type(value) not in (int, float) or not math.isfinite(value) or value < 0:
                    raise RuntimeError(f"Invalid native timing: {key}")
            if (not 8 <= final["wall_seconds"] <= 8.05 or final.get("start_monotonic_raw_ns") != start_ns
                    or abs(final["cpu_seconds"] - final["user_cpu_seconds"] - final["system_cpu_seconds"]) > 1e-6
                    or abs((final["end_monotonic_raw_ns"] - start_ns) / 1e9 - final["wall_seconds"]) > 1e-6):
                raise RuntimeError("Native fixed-window accounting failed")
            delta = after["stats"]["entries"] - before["stats"]["entries"]
            if (delta <= 0 or delta % 3 or after["stats"]["files"] - before["stats"]["files"] != delta // 3
                    or after["stats"]["directories"] - before["stats"]["directories"] != delta // 3 * 2
                    or after_journal["stored_scope_stats"].get("entries") != 3):
                raise RuntimeError("Observed work is not complete three-entry Changed scopes")
            for key in ("snapshot_read_attempts", "raw_snapshot_publications", "presentation_publications", "model_ui_invalidations",
                        "scanning_rising_edges", "scanning_falling_edges"):
                if type(final.get(key)) is not int or final[key] < 0:
                    raise RuntimeError("Missing native observer counts")
            if (final["scanning_rising_edges"] != final["scanning_falling_edges"]
                    or final["scanning_rising_edges"] + final["scanning_falling_edges"] > final["raw_snapshot_publications"]):
                raise RuntimeError("Observed scanning edges must balance between idle endpoints and fit the raw publications")
            if final["snapshot_read_attempts"] == 0 or final["raw_snapshot_publications"] == 0:
                raise RuntimeError("Real watcher work must reach ordinary model polling")
            energy.validate_state(state, root, 0)
            if digest(binary) != hashes["app"] or digest(archive) != hashes["core_archive"]:
                raise RuntimeError("A measured executable/archive changed")
            report = {"verified": True, "hardware": benchmark.hardware(), "command": command, "sha256": hashes,
                      "fixture": str(fixture), "marker": marker, "ready": ready, "native": final, "writes": writes,
                      "before_journal": before_journal, "endpoint_journal": endpoint_journal, "after_journal": after_journal,
                      "inferred_three_entry_scope_passes": delta // 3, "ledger_unchanged": True,
                      "method": "One eight-second native process CPU window with 16 absolute 400 ms same-inode manifest writes starting at +250 ms. Each pulse opens the existing no-follow file, verifies its identity, writes, fsyncs and closes within 50 ms of its deadline. Writer CPU is external. Exact final diagnostic timestamp, drained queues, frozen native/fresh-engine equality and a quiet tail are required, without repair polling. Input writes may be coalesced by FSEvents; inferred passes are measured entry deltas, not callback counts. Setup/audits warm caches. No profiler, cleanup, permission probing or user library is involved. Fresh fixture identities differ across invocations; each run preserves its own positive proof."}
        finally:
            benchmark.stop_process(process)
    audit_after = file_audit(root)
    coverage_after = benchmark.verify_fixture(root, marker["baseline"])
    write_json(output / "audit-after.json", {"files": audit_after, "coverage": coverage_after})
    if not coverage_after["matches_marker"] or set(audit_after) != set(audit_before):
        raise RuntimeError("Final fixture coverage changed")
    for relative, original in audit_before.items():
        expected = dict(original)
        if relative == CHANGED:
            expected.update(modifiedNs=writes[-1]["modified_ns"], changedNs=audit_after[relative]["changedNs"],
                            sha256=hashlib.sha256(manifest(116)).hexdigest())
        if audit_after[relative] != expected:
            raise RuntimeError(f"Fixture was not preserved: {relative}")
    if journal(database, root, writes[-1]["modified_ns"]) != after_journal:
        raise RuntimeError("Library proof changed at process shutdown")
    report.update(audit_before={"files": audit_before, "coverage": coverage_before},
                  audit_after={"files": audit_after, "coverage": coverage_after})
    write_json(output / "summary.json", report)
    print(json.dumps({"fixture": str(fixture), "wall_seconds": final["wall_seconds"],
                      "cpu_seconds": final["cpu_seconds"], "inferred_scope_passes": delta // 3}))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
