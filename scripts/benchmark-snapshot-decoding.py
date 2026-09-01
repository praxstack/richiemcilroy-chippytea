#!/usr/bin/env python3
"""Compile actual bridge decoders and compare synthetic in-memory responses.

This measures copying, decoding and identical correctness checks, not native
polling, FFI, scanning or filesystem work. Outputs and compiled inputs remain.
"""
from __future__ import annotations

import argparse
from datetime import datetime, timezone
import hashlib
import json
import math
from pathlib import Path
import platform
import re
import shutil
import statistics
import subprocess
import sys

import benchmark

REPO = Path(__file__).resolve().parents[1]
BOUNDARY = "private enum DiskAccessSetupRecord:"
CASES = ("nine-identical", "fivehundred-identical", "nine-changing", "fivehundred-changing", "oversized-identical")


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def digest(path: Path) -> str:
    value = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024**2), b""):
            value.update(block)
    return value.hexdigest()


def summary(records: list[dict]) -> dict:
    result = {}
    for case in CASES:
        result[case] = {}
        for variant in ("baseline", "candidate"):
            samples = [r for r in records if r["case"] == case and r["variant"] == variant and r.get("verified")]
            warm = [r["decoder"] for r in samples if r["phase"] == "warm"]
            cpu = [r["loop_user_cpu_seconds"] + r["loop_system_cpu_seconds"] for r in warm]
            wall = [r["loop_wall_ms"] for r in warm]
            result[case][variant] = {
                "verified_runs": len(samples),
                "first_loop_wall_ms": samples[0]["decoder"]["loop_wall_ms"] if samples else None,
                "warm_loop_wall_median_ms": statistics.median(wall) if wall else None,
                "warm_loop_wall_p95_ms": benchmark.percentile95(wall),
                "warm_loop_cpu_median_seconds": statistics.median(cpu) if cpu else None,
                "warm_loop_cpu_p95_seconds": benchmark.percentile95(cpu),
                "maximum_whole_process_rss_bytes": max((r["peak_rss_bytes"] for r in samples), default=None),
                "wire_bytes": samples[0]["decoder"]["wire_bytes"] if samples else None,
            }
    return result


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline-source", type=Path, default=REPO / "benchmarks/local/postscan-work/before/native/Chippytea/EngineBridge.swift")
    parser.add_argument("--candidate-source", type=Path, default=REPO / "native/Chippytea/EngineBridge.swift")
    parser.add_argument("--core-archive", type=Path, default=REPO / "target/release/libchippytea_core.a")
    parser.add_argument("--iterations", type=int, default=100)
    parser.add_argument("--warm-runs", type=int, default=6)
    parser.add_argument("--timeout", type=float, default=120)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    require(platform.system() == "Darwin" and platform.machine() == "arm64", "This benchmark targets Apple Silicon macOS")
    require(2 <= args.iterations <= 10_000 and 1 <= args.warm_runs <= 100 and 1 <= args.timeout <= 3600, "Invalid run bounds")
    output = args.output.resolve()
    require((REPO / "benchmarks/local").resolve() in output.parents, "Use a new directory under benchmarks/local")
    output.mkdir(parents=True, exist_ok=False)
    source = output / "source"
    source.mkdir()
    original = {
        "baseline": args.baseline_source.resolve(strict=True), "candidate": args.candidate_source.resolve(strict=True),
        "archive": args.core_archive.resolve(strict=True), "models": REPO / "native/Chippytea/Models.swift",
        "services": REPO / "native/Chippytea/NativeServices.swift", "package": REPO / "Package.swift",
        "header": REPO / "native/Bridge/chippytea.h", "modulemap": REPO / "native/Bridge/module.modulemap",
        "driver": Path(__file__).with_name("snapshot-decoding-benchmark.swift"),
        "runner": Path(__file__).resolve(), "helpers": Path(benchmark.__file__).resolve(),
    }
    report = {"protocol": 1, "verified": False, "started_at": datetime.now(timezone.utc).isoformat(),
        "hardware": benchmark.hardware(), "iterations": args.iterations, "warm_runs": args.warm_runs,
        "sources": {k: {"path": str(p), "sha256": digest(p)} for k, p in original.items()},
        "compilations": {}, "runs": [],
        "method": "Actual EngineBridge prefix, Models and NativeServices; same Rust archive linked but never opened or called. Synthetic snapshots only. Identical cases copy fresh bytes each iteration and include one initially empty cache miss; changing controls alternate two different equal-length responses; valid oversized responses bypass reuse. Loop timing includes copying, decode/reuse and full equality/precision validation; whole-process timing/RSS additionally includes startup and setup. First invocation is separate from alternating warm pairs, with no cold-cache claim. No FFI, scanner, UI, event, database or filesystem performance is measured. This harness never requests cleanup."}
    result_path = output / "summary.json"
    try:
        require(shutil.disk_usage(output).free >= 3 * 1024**3 + original["archive"].stat().st_size + 64 * 1024**2, "Keep a 3 GiB reserve plus compiler inputs")
        names = {"models": "Models.swift", "services": "NativeServices.swift", "package": "Package.swift",
                 "header": "Bridge/chippytea.h", "modulemap": "Bridge/module.modulemap", "archive": "Core/libchippytea_core.a",
                 "driver": "SnapshotDecodingBenchmark.swift", "runner": "benchmark-snapshot-decoding.py", "helpers": "benchmark.py"}
        for key, relative in names.items():
            target = source / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(original[key], target)
            require(digest(target) == report["sources"][key]["sha256"], "An input changed while being frozen")
        prefixes = {}
        for variant in ("baseline", "candidate"):
            text = original[variant].read_text()
            require(text.count(BOUNDARY) == 1, "EngineBridge extraction boundary changed")
            prefix = source / f"EngineBridge-{variant}.swift"
            prefix.write_text(text.split(BOUNDARY, 1)[0])
            prefixes[variant] = prefix
            report["sources"][variant]["prefix_sha256"] = digest(prefix)
        report["frozen_inputs"] = {str(path.relative_to(output)): digest(path)
            for path in [*(source / name for name in names.values()), *prefixes.values()]}
        package = (source / "Package.swift").read_text()
        match = re.search(r"\.unsafeFlags\(\[([^]]+)\]\)", package)
        require(match is not None and "swiftLanguageModes: [.v5]" in package and ".macOS(.v14)" in package, "Package compiler/linker configuration changed")
        linker = json.loads("[" + match[1] + "]")
        require(linker == ["-Ltarget/release", "-lchippytea_core", "-lsqlite3"], "Review changed Package linker flags")
        linker[0] = "-L" + str(source / "Core")
        frameworks = re.findall(r'\.linkedFramework\("([^"]+)"\)', package)
        require(frameworks == ["Security", "CoreServices"], "Review changed Package frameworks")
        for framework in frameworks:
            linker.extend(["-framework", framework])
        report["compiler_version"] = benchmark.command_text(["/usr/bin/xcrun", "swiftc", "--version"])
        binaries = {}
        for variant in ("baseline", "candidate"):
            binary = output / f"snapshot-decoder-{variant}"
            command = ["/usr/bin/xcrun", "swiftc", "-O", "-whole-module-optimization", "-parse-as-library",
                "-swift-version", "5", "-target", "arm64-apple-macosx14.0", "-module-name", "SnapshotDecodingBenchmark",
                "-module-cache-path", str(output / "module-cache"), "-I", str(source / "Bridge")]
            if variant == "candidate":
                command.extend(["-D", "SNAPSHOT_CACHE"])
            command.extend([str(prefixes[variant]), str(source / "Models.swift"), str(source / "NativeServices.swift"),
                            str(source / "SnapshotDecodingBenchmark.swift"), *linker, "-o", str(binary)])
            compilation = {"command": command, "stdout": f"compile-{variant}.stdout", "stderr": f"compile-{variant}.stderr"}
            report["compilations"][variant] = compilation
            with (output / compilation["stdout"]).open("xb") as out, (output / compilation["stderr"]).open("xb") as err:
                completed = subprocess.run(command, stdout=out, stderr=err, timeout=180, check=False)
            compilation["exit_code"] = completed.returncode
            require(completed.returncode == 0, f"{variant} compilation failed; retained compiler output")
            compilation["binary_sha256"] = digest(binary)
            binaries[variant] = binary
        require(all(digest(p) == report["sources"][k]["sha256"] for k, p in original.items()), "Source inputs changed before timing")
        require(all(digest(output / path) == value for path, value in report["frozen_inputs"].items()), "Frozen compiler inputs changed")
        proofs = {}
        for case in CASES:
            for number in range(args.warm_runs + 1):
                phase = "first" if number == 0 else "warm"
                order = ("baseline", "candidate") if number % 2 == 0 else ("candidate", "baseline")
                for variant in order:
                    require(shutil.disk_usage(output).free >= 3 * 1024**3, "Disk reserve fell below 3 GiB")
                    require(digest(binaries[variant]) == report["compilations"][variant]["binary_sha256"], "Frozen benchmark binary changed")
                    record = benchmark.run_sample(f"{case}-{variant}", phase, number,
                        [str(binaries[variant]), case, str(args.iterations)], output, args.timeout)
                    record.update(case=case, variant=variant, pair=number, verified=False)
                    report["runs"].append(record)
                    require(record["exit_code"] == 0 and not record["timed_out"] and not record["json_line_exceeded_1_mib"], "Decoder process failed")
                    raw = (output / record["stdout_artifact"]).read_bytes()
                    require(len(raw) <= 16 * 1024, "Unexpectedly large decoder output")
                    decoded = json.loads(raw)
                    record["decoder"] = decoded
                    require(decoded.get("protocol") == 1 and decoded.get("verified") is True
                            and decoded["variant"] == variant and decoded["case"] == case
                            and decoded["iterations"] == args.iterations and decoded["precision_checked"] is True
                            and decoded["cache_starts_empty"] is True, "Decoder correctness proof failed")
                    count = 500 if case.startswith("fivehundred") else 9
                    changing, oversized = case.endswith("changing"), case == "oversized-identical"
                    sizes = decoded["wire_bytes"]
                    hashes = decoded["wire_sha256"]
                    require(decoded["candidate_count"] == count and decoded["changing"] is changing and decoded["oversized"] is oversized
                            and len(sizes) == len(hashes) == 2 and sizes[0] == sizes[1]
                            and (sizes[0] > 1024**2) == oversized and (hashes[0] != hashes[1]) == changing,
                            "Synthetic input shape changed")
                    for key in ("loop_wall_ms", "loop_user_cpu_seconds", "loop_system_cpu_seconds"):
                        require(type(decoded[key]) in (int, float) and math.isfinite(decoded[key]) and decoded[key] >= 0, "Invalid interval timing")
                    require(type(record["peak_rss_bytes"]) is int and record["peak_rss_bytes"] > 0, "Missing lifetime process RSS")
                    proof = {key: decoded[key] for key in ("iterations", "candidate_count", "wire_bytes", "wire_sha256", "checksum")}
                    require(proofs.setdefault(case, proof) == proof, "Compared implementations received unequal inputs or decoded unequal results")
                    require(digest(binaries[variant]) == report["compilations"][variant]["binary_sha256"], "Benchmark binary changed during timing")
                    record["verified"] = True
                    result_path.write_text(json.dumps(report, indent=2) + "\n")
        require(all(digest(p) == report["sources"][k]["sha256"] for k, p in original.items()), "Source inputs changed during comparison")
        require(all(digest(output / path) == value for path, value in report["frozen_inputs"].items()), "Frozen compiler inputs changed during comparison")
        report["verified"] = True
    except (Exception, KeyboardInterrupt) as error:
        report["failure"] = f"{type(error).__name__}: {error}"
        print(report["failure"], file=sys.stderr)
    finally:
        report["completed_at"] = datetime.now(timezone.utc).isoformat()
        report["summary"] = summary(report["runs"])
        result_path.write_text(json.dumps(report, indent=2) + "\n")
    print(result_path)
    return 0 if report["verified"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
