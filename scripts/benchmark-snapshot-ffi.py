#!/usr/bin/env python3
"""Build against explicit frozen rlibs, then compare isolated snapshot workloads."""
from __future__ import annotations

import argparse
import hashlib
import json
import math
from pathlib import Path
import platform
import shutil
import statistics
import subprocess

SOURCE = Path(__file__).with_name("snapshot-ffi-benchmark.rs").resolve()


def require(value: bool, message: str) -> None:
    if not value:
        raise ValueError(message)


def digest(path: Path) -> str:
    value = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            value.update(block)
    return value.hexdigest()


def write_json(path: Path, data: object) -> None:
    with path.open("x") as stream:
        json.dump(data, stream, indent=2, allow_nan=False)
        stream.write("\n")


def build(args: argparse.Namespace) -> None:
    out = args.output.resolve()
    out.mkdir(mode=0o700)  # Refuse an existing output, never overwrite evidence.
    archive = args.rlib.resolve(strict=True)
    dependencies = args.dependencies.resolve(strict=True)
    original_hash = digest(archive)
    copied = out / "libchippytea_core.rlib"
    shutil.copyfile(archive, copied)
    require(digest(copied) == original_hash, "Rlib changed while being copied")
    externs = {"chippytea_core": copied}
    for crate in ("libc", "blake3", "serde_json", "rusqlite"):
        matches = list(dependencies.glob(f"lib{crate}-*.rlib"))
        require(len(matches) == 1, f"Expected exactly one {crate} rlib in {dependencies}")
        externs[crate] = matches[0]
    hashes = {str(path): digest(path) for path in dependencies.glob("*.rlib")}
    hashes[str(archive)] = original_hash
    hashes[str(copied)] = original_hash
    hashes[str(SOURCE)] = digest(SOURCE)
    command = ["rustc", "--edition=2024", "-C", "opt-level=3", "-C", "debuginfo=0",
               "-L", f"dependency={dependencies}"]
    for name, path in externs.items():
        command.extend(("--extern", f"{name}={path}"))
    command.extend((str(SOURCE), "-o", str(out / "driver")))
    with (out / "build.log").open("x") as log:
        subprocess.run(command, stdout=log, stderr=subprocess.STDOUT, timeout=180, check=True)
    require(all(digest(Path(path)) == value for path, value in hashes.items()),
            "Compilation inputs changed during the build")
    write_json(out / "build.json", {"protocol": 2, "command": command, "input_sha256": hashes,
                                    "binary_sha256": digest(out / "driver")})
    print(out / "driver", flush=True)


def measured_run(binary: Path, directory: Path, count: int, iterations: int) -> dict:
    command = [str(binary), str(directory), str(count), str(iterations)]
    with directory.with_suffix(".stdout.json").open("x") as stdout, \
            directory.with_suffix(".stderr.log").open("x") as stderr:
        subprocess.run(command, stdout=stdout, stderr=stderr, check=True, timeout=55)
    record = json.loads(directory.with_suffix(".stdout.json").read_text())
    require(record.get("protocol") == 2 and record.get("verified") is True
            and record.get("synthetic") is True, "Missing workload verification")
    require(record["candidates"] == count and record["receipts"] == 100
            and record["iterations"] == iterations, "Unexpected workload shape")
    require(record.get("timed_responses_validated") == iterations,
            "Not every timed response was validated")
    require(record["database_blake3_before"] == record["database_blake3_after"],
            "Library contents changed during snapshots")
    require(record == json.loads((directory / "result.json").read_text()),
            "Retained result differs from stdout")
    return record


def compare(args: argparse.Namespace) -> None:
    require(2 <= args.iterations <= 10_000 and 2 <= args.rounds <= 30, "Invalid bounded workload")
    out = args.output.resolve()
    out.mkdir(mode=0o700)
    binaries = {"before": args.before.resolve(strict=True), "after": args.after.resolve(strict=True)}
    binary_hashes = {name: digest(path) for name, path in binaries.items()}
    runs: list[dict] = []
    reference: dict[int, tuple[str, int]] = {}
    for count in (0, 500):
        # One unreported warm-up pair, then alternate execution order to limit
        # drift bias. Each sample uses a fresh process and a fresh seeded DB.
        for sample in range(-1, args.rounds):
            order = ("before", "after") if sample % 2 == 0 else ("after", "before")
            for name in order:
                label = "warmup" if sample == -1 else f"sample-{sample:02}"
                directory = out / f"{count:03}-{label}-{name}"
                record = measured_run(binaries[name], directory, count, args.iterations)
                identity = (record["canonical_response_blake3"], record["response_bytes"])
                require(identity == reference.setdefault(count, identity),
                        "First/last snapshot contents or size differ across variants/runs")
                runs.append({"variant": name, "sample": sample, **record})
                write_json(out / f"{directory.name}.verified.json", runs[-1])
                print(f"{count:3} candidates {label} {name}: "
                      f"{record['wall_seconds'] * 1000 / args.iterations:.3f} ms/call, "
                      f"{record['cpu_seconds'] * 1000 / args.iterations:.3f} CPU ms/call", flush=True)
    require(all(digest(binaries[name]) == value for name, value in binary_hashes.items()),
            "A driver binary changed during the comparison")
    summary = []
    for count in (0, 500):
        for name in binaries:
            selected = [record for record in runs
                        if record["candidates"] == count and record["variant"] == name and record["sample"] >= 0]
            metrics = {}
            for field in ("wall_seconds", "cpu_seconds", "user_seconds", "system_seconds"):
                values = sorted(record[field] * 1000 / args.iterations for record in selected)
                metrics[field.replace("_seconds", "_ms_per_call")] = {
                    "median": statistics.median(values),
                    "p95_sample_mean": values[math.ceil(0.95 * len(values)) - 1],
                }
            metrics["process_lifetime_peak_rss_bytes"] = {
                "median": statistics.median(record["peak_rss_after_bytes"] for record in selected),
                "max": max(record["peak_rss_after_bytes"] for record in selected),
            }
            summary.append({"candidates": count, "receipts": 100, "variant": name, **metrics})
    write_json(out / "summary.json", {
        "protocol": 2, "verified": True, "platform": platform.platform(),
        "binary_sha256": binary_hashes, "iterations": args.iterations, "rounds": args.rounds,
        "timed_response_validation": "Every timed response is compared byte-for-byte with that variant's validated successful reference",
        "summary": summary, "runs": runs,
        "limits": [
            "Synthetic warm Rust FFI snapshot workload; not discovery, Swift decode, UI, or user cleanup latency",
            "Timings include C-string access, full byte-for-byte response comparison and validation counting in addition to ct_request/ct_free_string",
            "p95 is across sample means, not a per-request latency distribution",
            "RSS is process-lifetime high-water including synthetic setup/JSON validation and one retained raw reference response, not timed-only allocation",
            "Baseline/final rule-version metadata may differ; response equivalence and DB immutability are checked separately",
        ],
    })
    print(out / "summary.json", flush=True)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="action", required=True)
    builder = subparsers.add_parser("build")
    builder.add_argument("--rlib", type=Path, required=True)
    builder.add_argument("--dependencies", type=Path, required=True)
    builder.add_argument("--output", type=Path, required=True)
    builder.set_defaults(run=build)
    runner = subparsers.add_parser("compare")
    runner.add_argument("--before", type=Path, required=True)
    runner.add_argument("--after", type=Path, required=True)
    runner.add_argument("--output", type=Path, required=True)
    runner.add_argument("--iterations", type=int, default=200)
    runner.add_argument("--rounds", type=int, default=6)
    runner.set_defaults(run=compare)
    args = parser.parse_args()
    args.run(args)


if __name__ == "__main__":
    main()
