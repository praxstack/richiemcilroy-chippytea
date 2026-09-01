#!/usr/bin/env python3
"""Compare actual Rust permanent-cleanup phases on freshly generated local fixtures.

The driver creates its own deletion target; no fixture path is accepted. Every
fixture, failed run and evidence file is retained. Generation/discovery/audits
warm caches and are outside the reported cleanup interval. This does not measure
Swift, FSEvents, or an end-user cleanup interaction.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import re
import shutil
import signal
import sqlite3
import stat
import statistics
import subprocess
import time

REPO = Path(__file__).resolve().parents[1]
DRIVER = REPO / "scripts/cleanup-benchmark.rs"
PAYLOAD = 100 * 1024**2
LEAF = b"Disposable compiled output; no user data.\n"
TAG = b"Signature: 8a477f597d28d172789f06886806bc55\n"
MAGIC = b"chippytea-cleanup-benchmark-v1\n"
PHASES = ("preflight", "preparing", "staged_checking", "removing", "accounting")
TIMINGS = ("wall_seconds", "user_seconds", "system_seconds", "cpu_seconds")
NOFOLLOW_ANY = 0x20000000  # macOS fcntl.h; not exposed by every Python build.


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def digest(path: Path) -> str:
    value = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024**2), b""):
            value.update(block)
    return value.hexdigest()


def reserve(leaves: int) -> None:
    required = 3 * 1024**3 + PAYLOAD + (leaves + 1024) * 16_384
    require(shutil.disk_usage("/private/tmp").free >= required,
            "Keep a 3 GiB reserve beyond the payload, metadata and audit budget")


def write_json(path: Path, value: object) -> None:
    with path.open("w") as stream:
        json.dump(value, stream, indent=2, allow_nan=False)
        stream.write("\n")


def compile_driver(name: str, archive: Path, dependencies: Path, source: Path, output: Path) -> dict:
    original = digest(archive)
    copied = output / f"lib{name}_core.rlib"
    shutil.copyfile(archive, copied)
    require(digest(copied) == original, "Rlib changed while copying")
    externs = {"chippytea_core": copied}
    for crate in ("libc", "blake3", "serde_json"):
        matches = list(dependencies.glob(f"lib{crate}-*.rlib"))
        require(len(matches) == 1, f"Provide a dependency directory containing one {crate} rlib")
        externs[crate] = matches[0]
    hashes = {str(path): digest(path) for path in externs.values()}
    command = ["rustc", "--edition=2024", "-C", "opt-level=3", "-C", "debuginfo=0",
               "-L", f"dependency={dependencies}"]
    for crate, path in externs.items():
        command.extend(("--extern", f"{crate}={path}"))
    binary = output / f"{name}-driver"
    command.extend((str(source), "-o", str(binary)))
    with (output / f"{name}-build.log").open("x") as log:
        result = subprocess.run(command, stdout=log, stderr=subprocess.STDOUT, timeout=180)
    result.check_returncode()
    require(digest(archive) == original and all(digest(Path(path)) == value for path, value in hashes.items()),
            "An input rlib changed during compilation")
    return {"command": command, "original_rlib": str(archive), "rlib_sha256": original,
            "extern_sha256": hashes, "binary_sha256": digest(binary)}


def run_driver(command: list[str], prefix: Path, timeout: float) -> tuple[dict, float]:
    start = time.monotonic()
    with prefix.with_suffix(".stdout.json").open("x") as stdout, prefix.with_suffix(".stderr.log").open("x") as stderr:
        process = subprocess.Popen(command, stdout=stdout, stderr=stderr, start_new_session=True)
        try:
            status = process.wait(timeout=timeout)
        except BaseException:
            # Stop only the process group created above. Retain every fixture,
            # manifest and recovery directory; never perform fallback deletion.
            try:
                os.killpg(process.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                try:
                    os.killpg(process.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                process.wait(timeout=10)
            raise
    require(status == 0, f"Driver exited {status}; inspect {prefix.with_suffix('.stderr.log')}. Fixture evidence is retained")
    return json.loads(prefix.with_suffix(".stdout.json").read_text()), time.monotonic() - start


def checked_bytes(path: Path, size: int) -> bytes:
    with os.fdopen(os.open(path, os.O_RDONLY | os.O_CLOEXEC | NOFOLLOW_ANY), "rb") as stream:
        info = os.fstat(stream.fileno())
        require(stat.S_ISREG(info.st_mode) and info.st_uid == os.geteuid() and info.st_nlink == 1
                and info.st_flags == 0 and info.st_size == size, "Preserved file identity/type changed")
        return stream.read(size + 1)


def validate(record: dict, case: str, leaves: int) -> dict:
    require(record.get("protocol") == 1 and record.get("verified") is True
            and record.get("case") == case and record.get("leaves") == leaves, "Unexpected driver result")
    fixture = Path(record["fixture"])
    require(fixture.parent == Path("/private/tmp") and re.fullmatch(r"chippytea-cleanup-[0-9a-f]{32}", fixture.name)
            and fixture.resolve(strict=True) == fixture, "Driver did not use its own physical disposable fixture")
    info = fixture.lstat()
    require(stat.S_ISDIR(info.st_mode) and stat.S_IMODE(info.st_mode) == 0o700
            and info.st_uid == os.geteuid() and info.st_flags == 0, "Fixture is not owned local mode0700 data")
    require(checked_bytes(fixture / ".chippytea-cleanup-fixture", len(MAGIC)) == MAGIC,
            "Missing exact disposable fixture marker")
    require(json.loads((fixture / "result.json").read_text()) == record, "Retained result differs from stdout")
    for key in ("source_and_sentinels_preserved", "artifact_absent", "staging_and_recovery_absent", "restart_and_duplicate_reward_checks"):
        require(record.get(key) is True, f"Missing completed safety audit: {key}")
    project = fixture / "Projects/Disposable"
    require({path.name for path in project.iterdir()} == {"Cargo.toml", "source.rs"}
            and {path.name for path in (fixture / "Projects").iterdir()} == {"Disposable", "Sibling"}
            and not os.path.lexists(project / "target"), "Artifact or unexpected stage/recovery data remains")
    preserved = {
        "Projects/Disposable/Cargo.toml": b'[package]\nname="disposable-cleanup"\nversion="0.1.0"\n',
        "Projects/Disposable/source.rs": b"Preserve this disposable sentinel exactly.\n",
        "Projects/Sibling/preserve.txt": b"Preserve this disposable sentinel exactly.\n",
        "outside-authorized-root.txt": b"Preserve this disposable sentinel exactly.\n",
        ".chippytea-cleanup-fixture": MAGIC,
    }
    captured = json.loads((fixture / "input.json").read_text())
    require(len(captured["sentinels"]) == len(preserved), "Missing initial sentinel identities")
    for (relative, expected), initial in zip(preserved.items(), captured["sentinels"]):
        path = fixture / relative
        require(checked_bytes(path, len(expected)) == expected, "Source or sentinel contents changed")
        value = path.lstat()
        actual = {"device": value.st_dev, "inode": value.st_ino, "mode": value.st_mode, "size": value.st_size,
                  "allocated_bytes": value.st_blocks * 512, "links": value.st_nlink, "uid": value.st_uid,
                  "flags": value.st_flags, "directory": False, "modified_ns": value.st_mtime_ns, "changed_ns": value.st_ctime_ns}
        require(actual == initial["metadata"], "Source or sentinel metadata changed")
    unique = leaves // 2 if case == "hardlinks" else leaves
    expected = {"entries": leaves + leaves // 256 + 4, "files": leaves + 2, "directories": leaves // 256 + 2,
                "unique_regular_files": unique + 2, "hardlink_groups": leaves // 2 if case == "hardlinks" else 0,
                "logical_bytes": PAYLOAD + unique * len(LEAF) + len(TAG)}
    require(all(record["input"][key] == value for key, value in expected.items())
            and record["input"]["allocated_bytes"] >= PAYLOAD and captured["artifact"] == record["input"],
            "Input counts or unique allocation differ from the declared workload")
    with (fixture / "input-metadata.jsonl").open() as stream:
        require(sum(1 for _ in stream) == expected["entries"], "Full input metadata audit is incomplete")
    require(record["discovery"]["complete"] is True and record["discovery"]["cancelled"] is False
            and record["discovery"]["errors"] == 0 and record["discovery"]["candidates"] == 1,
            "Ordinary discovery did not establish one genuine candidate")
    require(tuple(phase["phase"] for phase in record["phases"]) == PHASES, "Unexpected actual phase boundaries")
    for phase in record["phases"]:
        count = 1 if phase["phase"] in ("preflight", "accounting") else expected["entries"]
        require(phase["completed"] == phase["total"] == count and phase["callbacks"] > 0,
                "A timed phase did not complete the declared artifact")
    for interval in [record["timing"], *(phase["timing"] for phase in record["phases"])]:
        require(all(isinstance(interval[key], (int, float)) and math.isfinite(interval[key]) and interval[key] >= -1e-9 for key in TIMINGS)
                and interval["wall_seconds"] > 0 and interval["lifetime_peak_rss_bytes"] > 0, "Invalid timing or RSS observation")
    for key in TIMINGS:
        require(abs(sum(phase["timing"][key] for phase in record["phases"]) - record["timing"][key]) < 1e-6,
                "Phase timings do not cover the complete cleanup interval")
    database = fixture / "State/library.sqlite"
    require(record["database"] == str(database) and database.resolve(strict=True) == database, "Result names another library")
    with sqlite3.connect(f"file:{database}?mode=ro", uri=True) as connection:
        require(connection.execute("SELECT count(*) FROM operations WHERE state='removed'").fetchone()[0] == 1
                and connection.execute("SELECT count(*) FROM operations").fetchone()[0] == 1
                and connection.execute("SELECT count(*) FROM cleanup_entries").fetchone()[0] == 0,
                "Completed operation or empty cleanup manifest was not durable")
        wallet = connection.execute("SELECT collected,remainder,credited FROM wallet WHERE id=1").fetchone()
        receipt = record["receipt"]
        require(wallet == (receipt["coins"], receipt["credited_bytes"] % 100_000_000, receipt["credited_bytes"])
                and receipt["coins"] == receipt["credited_bytes"] // 100_000_000
                and receipt["credited_bytes"] <= min(receipt["observed_bytes"], receipt["reported_bytes"])
                and receipt["reported_bytes"] == record["input"]["allocated_bytes"]
                and connection.execute("SELECT coalesce(sum(coins),0) FROM earnings WHERE collected=0").fetchone()[0] == 0,
                "Restarted wallet or conservative receipt bounds are inconsistent")
    return {"case": case, "leaves": leaves, **expected, "allocated_bytes": record["input"]["allocated_bytes"],
            "phase_completed_total": [[value["phase"], value["completed"], value["total"]] for value in record["phases"]]}


def summary(samples: list[float]) -> dict:
    return {"samples": samples, "median": statistics.median(samples),
            "p95": sorted(samples)[math.ceil(len(samples) * 0.95) - 1]}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True, help="New evidence directory; must not exist")
    parser.add_argument("--before-rlib", type=Path, default=REPO / "benchmarks/local/cleanup-throughput/libchippytea_core.rlib")
    parser.add_argument("--after-rlib", type=Path, default=REPO / "target/release/libchippytea_core.rlib")
    parser.add_argument("--deps-dir", type=Path, default=REPO / "target/release/deps")
    parser.add_argument("--cases", nargs="+", choices=("regular", "hardlinks"), default=["regular", "hardlinks"])
    parser.add_argument("--leaves", type=int, default=16_384)
    parser.add_argument("--warm-runs", type=int, default=3)
    parser.add_argument("--timeout-seconds", type=float, default=240)
    parser.add_argument("--ready-delay-ms", type=int, default=0, help="Untimed attach delay, at most5000ms")
    args = parser.parse_args()
    require(platform.system() == "Darwin" and Path("/private/tmp").resolve(strict=True) == Path("/private/tmp"), "Run on macOS with physical /private/tmp")
    require(256 <= args.leaves <= 131_072 and args.leaves % 256 == 0 and 0 <= args.warm_runs <= 20
            and len(set(args.cases)) == len(args.cases) and 1 <= args.timeout_seconds <= 3600
            and 0 <= args.ready_delay_ms <= 5000, "Invalid case, count, timeout or delay bounds")
    output = args.output.expanduser().absolute()
    require(not os.path.lexists(output), "Refusing an existing output directory")
    output.mkdir(mode=0o700, parents=True)
    source_hashes = {str(DRIVER): digest(DRIVER), str(Path(__file__).resolve()): digest(Path(__file__).resolve())}
    source = output / "cleanup-benchmark.rs"
    shutil.copyfile(DRIVER, source)
    dependencies = args.deps_dir.resolve(strict=True)
    archives = {"before": args.before_rlib.resolve(strict=True), "after": args.after_rlib.resolve(strict=True)}
    report = {"protocol": 1, "verified": False, "source_sha256": source_hashes, "leaves": args.leaves,
              "warm_pairs_per_case": args.warm_runs, "cases": args.cases, "runs": [], "builds": {},
              "hardware": {"platform": platform.platform(), "machine": platform.machine(), "python": platform.python_version()},
              "scope": "Fresh independently written fixtures per invocation. One separate first pair, then alternating warm pairs. Setup and full audits warm caches; these are not cold disk timings. Cleanup timing comes from actual Rust progress boundaries. Generation, scanning, audits, restart and collection are excluded. No native UI or FSEvents. RSS includes lifetime setup/audit allocations. Accounting is ambient-dependent and zero credit is accepted."}
    report_path = output / "summary.json"
    try:
        hardware = subprocess.run(["/usr/sbin/sysctl", "-n", "machdep.cpu.brand_string", "hw.memsize", "hw.ncpu"], capture_output=True, text=True, check=True)
        report["hardware"]["cpu_memory_bytes_cpu_count"] = hardware.stdout.splitlines()
        report["rustc"] = subprocess.run(["rustc", "--version", "--verbose"], capture_output=True, text=True, check=True).stdout
        for name, archive in archives.items():
            report["builds"][name] = compile_driver(name, archive, dependencies, source, output)
        require(digest(source) == source_hashes[str(DRIVER)], "Frozen common driver source changed")
        fixtures: set[str] = set()
        serial = 0
        for case in args.cases:
            reference = None
            for pair in range(args.warm_runs + 1):
                for name in (("before", "after") if pair % 2 == 0 else ("after", "before")):
                    reserve(args.leaves)
                    serial += 1
                    binary = output / f"{name}-driver"
                    require(digest(binary) == report["builds"][name]["binary_sha256"], "Frozen executable changed")
                    command = [str(binary), "--case", case, "--leaves", str(args.leaves), "--ready-delay-ms", str(args.ready_delay_ms)]
                    prefix = output / f"{serial:03}-{case}-{name}"
                    record, process_wall = run_driver(command, prefix, args.timeout_seconds)
                    proof = validate(record, case, args.leaves)
                    require(record["fixture"] not in fixtures, "Each invocation must create a fresh fixture")
                    fixtures.add(record["fixture"])
                    reference = reference or proof
                    require(proof == reference, "Paired fixture counts, unique bytes, allocated bytes or completed phases differ")
                    report["runs"].append({"variant": name, "case": case, "pair": pair, "phase": "first" if pair == 0 else "warm",
                        "command": command, "evidence_prefix": prefix.name, "fixture": record["fixture"], "verified": True,
                        "process_wall_seconds_including_setup": process_wall, "timing": record["timing"], "phases": record["phases"],
                        "proof": proof, "receipt": record["receipt"]})
                    write_json(report_path, report)
        report["warm_summary"] = {}
        if args.warm_runs:
            for case in args.cases:
                report["warm_summary"][case] = {}
                for name in archives:
                    runs = [run for run in report["runs"] if run["case"] == case and run["variant"] == name and run["phase"] == "warm"]
                    values = {"total": {key: summary([run["timing"][key] for run in runs]) for key in TIMINGS},
                              "lifetime_peak_rss_bytes": summary([run["timing"]["lifetime_peak_rss_bytes"] for run in runs])}
                    for index, phase in enumerate(PHASES):
                        values[phase] = {key: summary([run["phases"][index]["timing"][key] for run in runs]) for key in TIMINGS}
                    report["warm_summary"][case][name] = values
        require(all(digest(Path(path)) == value for path, value in source_hashes.items()), "Harness sources changed during comparison")
        for name, build in report["builds"].items():
            require(digest(archives[name]) == build["rlib_sha256"]
                    and all(digest(Path(path)) == value for path, value in build["extern_sha256"].items()), "An rlib changed during comparison")
        report["verified"] = True
    except BaseException as error:
        report["error"] = f"{type(error).__name__}: {error}"
        raise
    finally:
        write_json(report_path, report)
    print(report_path)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
