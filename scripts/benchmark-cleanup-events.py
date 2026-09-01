#!/usr/bin/env python3
"""Compare production FolderWatcher retention during generated-only Rust cleanup.

One identical Rust child is used for both frozen NativeServices.swift variants.
The Swift observer retains callback arrays until child exit plus a quiet tail;
it does not launch Chippytea or run AppModel/the full engine request queue.
"""
from __future__ import annotations

import argparse
import importlib.util
import json
import math
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys

REPO = Path(__file__).resolve().parents[1]
OBSERVER = REPO / "scripts/cleanup-events-benchmark.swift"
HELPER = REPO / "scripts/benchmark-cleanup.py"
CONTROL = b"Disposable watcher control; preserve this file.\n"

# Reuse the generated-only driver's compiler and independent filesystem/ledger
# audit. Importing the helper must not create source-adjacent bytecode files.
sys.dont_write_bytecode = True
SPEC = importlib.util.spec_from_file_location("chippytea_cleanup_benchmark", HELPER)
assert SPEC is not None and SPEC.loader is not None
cleanup = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(cleanup)
require = cleanup.require


def instrument_history_loss(name: str, original: Path, output: Path) -> tuple[Path, dict]:
    """Instrument only a frozen service copy, rejecting unknown callback layouts."""
    source = original.read_bytes().decode("utf-8")
    signature = "        let callback: FSEventStreamCallback = { _, info, count, paths, flags, ids in\n"
    receiver = "            let watcher = Unmanaged<FolderWatcher>.fromOpaque(info).takeUnretainedValue()\n"
    creation = "        stream = FSEventStreamCreate(nil, callback, &context, paths as CFArray,\n"
    layouts = (
        ("legacy", "            watcher.handler(changed, last, historyLost)\n", "historyLost", "last"),
        ("decoded_batch", "                watcher.handler(batch.events, batch.last, batch.historyLost)\n", "batch.historyLost", "batch.last"),
    )
    require(all(source.count(anchor) == 1 for anchor in (signature, receiver, creation))
            and sum(source.count(layout[1]) for layout in layouts) == 1,
            f"Unsupported {name} FolderWatcher diagnostic layout; originals are preserved")
    layout, anchor, condition, last = next(layout for layout in layouts if source.count(layout[1]) == 1)
    require(source.index(signature) < source.index(receiver) < source.index(anchor) < source.index(creation),
            "History-loss dispatch is outside the expected callback")
    block = """// Benchmark-only diagnosis; this source is never installed as the app.
if __CONDITION__ {
    let diagnosticPaths = unsafeBitCast(paths, to: NSArray.self)
    var diagnosticFlags: FSEventStreamEventFlags = 0
    for index in 0..<count { diagnosticFlags |= flags[index] }
    var diagnosticNonStrings = 0
    for value in diagnosticPaths {
        if (value as? String) == nil { diagnosticNonStrings += 1 }
    }
    let diagnosticLossBits = diagnosticFlags & FSEventStreamEventFlags(kFSEventStreamEventFlagUserDropped | kFSEventStreamEventFlagKernelDropped | kFSEventStreamEventFlagEventIdsWrapped)
    let diagnostic: [String: Any] = ["chippytea_history_loss_diagnostic": 1,
        "variant": __VARIANT__, "layout": __LAYOUT__, "flags_or": diagnosticFlags,
        "os_loss_bits": diagnosticLossBits, "callback_count": count,
        "nsarray_count": diagnosticPaths.count, "non_string_path_count": diagnosticNonStrings,
        "last_event_id": __LAST__]
    if let data = try? JSONSerialization.data(withJSONObject: diagnostic, options: [.sortedKeys]) {
        FileHandle.standardError.write(data + Data([10]))
    }
}
"""
    block = (block.replace("__CONDITION__", condition).replace("__LAST__", last)
             .replace("__VARIANT__", json.dumps(name)).replace("__LAYOUT__", json.dumps(layout)))
    indent = anchor[:len(anchor) - len(anchor.lstrip())]
    inserted = "".join(indent + line + "\n" for line in block.splitlines())
    target = output / f"{name}_services_diagnostic.swift"
    with target.open("xb") as stream:
        stream.write(source.replace(anchor, inserted + anchor, 1).encode("utf-8"))
    return target, {"layout": layout, "dispatch_anchor": anchor.strip(),
                    "original_copy": str(original), "original_sha256": cleanup.digest(original),
                    "instrumented_copy": str(target), "instrumented_sha256": cleanup.digest(target),
                    "stderr_record_key": "chippytea_history_loss_diagnostic"}


def compile_observer(name: str, services: Path, observer: Path, output: Path, sdk: str) -> dict:
    binary = output / f"{name}-observer"
    command = ["/usr/bin/xcrun", "swiftc", "-O", "-whole-module-optimization", "-parse-as-library",
               "-swift-version", "5", "-target", f"{platform.machine()}-apple-macosx14.0", "-sdk", sdk,
               "-module-cache-path", str(output / f"{name}-module-cache"),
               "-module-name", "ChippyteaCleanupEventsBenchmark", str(services), str(observer)]
    for framework in ("AppKit", "AVFoundation", "CoreServices", "QuickLookUI"):
        command.extend(("-framework", framework))
    command.extend(("-o", str(binary)))
    with (output / f"{name}-build.log").open("x") as log:
        subprocess.run(command, stdout=log, stderr=subprocess.STDOUT, timeout=180, check=True)
    return {"command": command, "binary_sha256": cleanup.digest(binary),
            "services_sha256": cleanup.digest(services), "observer_sha256": cleanup.digest(observer)}


def validate_observer(record: dict, raw: Path, driver: Path, case: str, leaves: int) -> tuple[dict, dict]:
    require(record.get("protocol") == 1 and record.get("verified") is True
            and record.get("case") == case and record.get("leaves") == leaves,
            "Unexpected watcher observer result")
    require(record["raw_directory"] == str(raw) and record["rust_driver"] == str(driver)
            and record["child_exit_status"] == 0 and record["artifact_present_when_watcher_started"] is True
            and 0 <= record["watcher_attach_ms"] < 800 and record["quiet_tail_seconds"] >= 1.2
            and record["quiet_tail_limit_seconds"] == 15
            and record["child_result_checked_before_completion_control"] is True
            and math.isfinite(record["completion_control_after_child_exit_ms"])
            and 0 <= record["completion_control_after_child_exit_ms"] < 15_000,
            "Observer did not surround the generated cleanup")
    child = json.loads((raw / "child.stdout.json").read_text())
    proof = cleanup.validate(child, case, leaves)
    require(record["fixture"] == child["fixture"], "Observer watched a different fixture")
    fixture = Path(child["fixture"])
    root = str(fixture / "Projects")
    control = fixture / "Projects/Sibling/watcher-control.txt"
    completion_control = fixture / "Projects/Sibling/watcher-complete-control.txt"
    for key, path in (("control_path", control), ("completion_control_path", completion_control)):
        require(record[key] == str(path) and cleanup.checked_bytes(path, len(CONTROL)) == CONTROL,
                "An ordinary watcher control was not preserved")
    callbacks = events = path_bytes = internal_events = max_id = 0
    control_seen = completion_control_seen = False
    with (raw / "events.jsonl").open() as stream:
        for line in stream:
            batch = json.loads(line)
            callbacks += 1
            require(batch["history_lost"] is False and isinstance(batch["last"], int) and batch["last"] > 0,
                    "Watcher lost history or did not deliver an event cursor")
            max_id = max(max_id, batch["last"])
            for event in batch["events"]:
                path = event["path"]
                require((path == root or path.startswith(root + "/"))
                        and event["kind"] in ("file", "directory", "unknown")
                        and isinstance(event["recursive"], bool), "Unexpected callback path or payload")
                events += 1
                path_bytes += len(path.encode("utf-8"))
                internal_events += path.startswith(root + "/Disposable/.chippytea-")
                control_seen |= path == str(control) and event["kind"] == "file"
                completion_control_seen |= path == str(completion_control) and event["kind"] == "file"
    audited = {"handler_callback_count": callbacks, "retained_batch_count": callbacks,
               "retained_event_count": events, "retained_path_utf8_bytes": path_bytes,
               "retained_internal_event_count": internal_events, "maximum_event_id": max_id,
               "history_lost": False, "ordinary_control_event_seen": control_seen,
               "completion_control_event_seen": completion_control_seen}
    require(audited == record["events"] and control_seen and completion_control_seen and max_id > 0,
            "Retained event evidence differs from observer counters or missed an ordinary control")
    timing = record["observer_timing"]
    require(all(isinstance(timing[key], (int, float)) and math.isfinite(timing[key])
                and timing[key] >= -1e-9 for key in cleanup.TIMINGS)
            and timing["wall_seconds"] > 0 and timing["lifetime_peak_rss_bytes_at_retention_end"] > 0
            and abs(timing["cpu_seconds"] - timing["user_seconds"] - timing["system_seconds"]) < 1e-6,
            "Invalid observer timing or lifetime RSS")
    return proof, child


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True, help="New evidence directory; must not exist")
    parser.add_argument("--rust-rlib", type=Path, default=REPO / "target/release/libchippytea_core.rlib")
    parser.add_argument("--deps-dir", type=Path, default=REPO / "target/release/deps")
    parser.add_argument("--before-services", type=Path,
                        default=REPO / "benchmarks/local/cleanup-throughput/before/native/Chippytea/NativeServices.swift")
    parser.add_argument("--after-services", type=Path, default=REPO / "native/Chippytea/NativeServices.swift")
    parser.add_argument("--cases", nargs="+", choices=("regular", "hardlinks"), default=["regular"])
    parser.add_argument("--leaves", type=int, default=16_384)
    parser.add_argument("--warm-runs", type=int, default=3)
    parser.add_argument("--timeout-seconds", type=float, default=240)
    parser.add_argument("--diagnose-history-loss", action="store_true",
                        help="Instrument frozen benchmark copies to report raw loss flags; diagnostic only, no performance conclusions")
    args = parser.parse_args()
    require(platform.system() == "Darwin" and platform.machine() in ("arm64", "x86_64")
            and Path("/private/tmp").resolve(strict=True) == Path("/private/tmp"), "Use macOS and physical /private/tmp")
    require(256 <= args.leaves <= 131_072 and args.leaves % 256 == 0 and 0 <= args.warm_runs <= 20
            and len(set(args.cases)) == len(args.cases) and 5 <= args.timeout_seconds <= 3600,
            "Invalid workload or timeout bounds")
    output = args.output.expanduser().absolute()
    require(not os.path.lexists(output), "Refusing an existing evidence directory")
    output.mkdir(mode=0o700, parents=True)
    originals = {"python": Path(__file__).resolve(), "helper": HELPER, "rust": cleanup.DRIVER, "observer": OBSERVER,
                 "before_services": args.before_services.resolve(strict=True), "after_services": args.after_services.resolve(strict=True)}
    source_hashes = {str(path): cleanup.digest(path) for path in originals.values()}
    frozen = {}
    for key, path in originals.items():
        target = output / f"{key}{path.suffix}"
        shutil.copyfile(path, target)
        require(cleanup.digest(target) == source_hashes[str(path)], "Source changed while freezing")
        frozen[key] = target
    report = {"protocol": 1, "verified": False, "source_sha256": source_hashes, "cases": args.cases,
              "leaves": args.leaves, "warm_pairs_per_case": args.warm_runs, "runs": [], "builds": {},
              "hardware": {"platform": platform.platform(), "machine": platform.machine(), "python": platform.python_version()},
              "scope": "One identical Rust cleanup child for both native variants. Fresh generated fixture per invocation; first pair then alternating warm pairs. Production FolderWatcher retains handler arrays until child completion, a distinct ordinary control and a 1.2s quiet tail, with a 15s post-child deadline. Both initial and post-cleanup controls must be delivered and preserved. Observer timing includes both control creations, completed-child result checks and waiting for the child's post-cleanup audits. RUSAGE_SELF excludes Rust and diskutil child CPU. Observer RSS is lifetime peak at retention end including setup, not Rust memory. Event serialization and final observer audits are outside observer timing. No AppModel, full engine request queue or UI. FSEvents coalescing varies between runs; counts need not match. Child cleanup timings are reported separately and do not by themselves establish a native-filter throughput gain."}
    report_path = output / "summary.json"
    try:
        services = {name: frozen[f"{name}_services"] for name in ("before", "after")}
        if args.diagnose_history_loss:
            report.update(diagnostic_only=True, diagnostic_mode="history_loss", diagnostic_sources={}, diagnostic_attempts=[])
            report["scope"] = ("Diagnostic only. Frozen service copies log raw callback shape and flags on history loss. "
                               "Instrumentation changes callback work; raw timings are retained only as context and support no performance claims. "
                               + report["scope"])
            for name in services:
                services[name], report["diagnostic_sources"][name] = instrument_history_loss(name, services[name], output)
        sdk = subprocess.run(["/usr/bin/xcrun", "--sdk", "macosx", "--show-sdk-path"], capture_output=True, text=True, check=True).stdout.strip()
        report["sdk"] = sdk
        report["swiftc"] = subprocess.run(["/usr/bin/xcrun", "swiftc", "--version"], capture_output=True, text=True, check=True).stdout
        report["rustc"] = subprocess.run(["rustc", "--version", "--verbose"], capture_output=True, text=True, check=True).stdout
        report["hardware"]["cpu_memory_bytes_cpu_count"] = subprocess.run(
            ["/usr/sbin/sysctl", "-n", "machdep.cpu.brand_string", "hw.memsize", "hw.ncpu"],
            capture_output=True, text=True, check=True).stdout.splitlines()
        archive = args.rust_rlib.resolve(strict=True)
        report["builds"]["rust"] = cleanup.compile_driver("cleanup", archive, args.deps_dir.resolve(strict=True), frozen["rust"], output)
        driver = output / "cleanup-driver"
        for name in ("before", "after"):
            report["builds"][name] = compile_observer(name, services[name], frozen["observer"], output, sdk)
        fixtures: set[str] = set()
        serial = 0
        for case in args.cases:
            reference = None
            for pair in range(args.warm_runs + 1):
                for name in (("before", "after") if pair % 2 == 0 else ("after", "before")):
                    cleanup.reserve(args.leaves)
                    serial += 1
                    binary = output / f"{name}-observer"
                    require(cleanup.digest(binary) == report["builds"][name]["binary_sha256"]
                            and cleanup.digest(driver) == report["builds"]["rust"]["binary_sha256"], "A frozen executable changed")
                    prefix = output / f"{serial:03}-{case}-{name}"
                    raw = output / f"{prefix.name}-child"
                    raw.mkdir(mode=0o700)
                    command = [str(binary), "--driver", str(driver), "--raw-dir", str(raw), "--case", case,
                               "--leaves", str(args.leaves), "--timeout-seconds", str(args.timeout_seconds)]
                    if args.diagnose_history_loss:
                        report["diagnostic_attempts"].append({"variant": name, "case": case, "pair": pair,
                            "command": command, "stderr_path": str(prefix.with_suffix(".stderr.log"))})
                    record, process_wall = cleanup.run_driver(command, prefix, args.timeout_seconds + 10)
                    proof, child = validate_observer(record, raw, driver, case, args.leaves)
                    require(record["fixture"] not in fixtures, "Each observer must receive a fresh generated fixture")
                    fixtures.add(record["fixture"])
                    reference = reference or proof
                    require(proof == reference, "Native variants received unequal cleanup workloads")
                    require(record["events"]["retained_internal_event_count"] > 0 if name == "before"
                            else record["events"]["retained_internal_event_count"] == 0,
                            "The control did not exercise internal cleanup traffic, or the new producer retained it")
                    report["runs"].append({"variant": name, "case": case, "pair": pair, "phase": "first" if pair == 0 else "warm",
                        "command": command, "evidence_prefix": prefix.name, "fixture": record["fixture"], "verified": True,
                        "process_wall_seconds_including_setup": process_wall, "observer_timing": record["observer_timing"],
                        "events": record["events"], "watcher_attach_ms": record["watcher_attach_ms"], "quiet_tail_seconds": record["quiet_tail_seconds"],
                        "completion_control_after_child_exit_ms": record["completion_control_after_child_exit_ms"],
                        "child_cleanup_timing": child["timing"], "child_cleanup_phases": child["phases"], "proof": proof})
                    cleanup.write_json(report_path, report)
        report["warm_summary"] = {}
        if args.warm_runs and not args.diagnose_history_loss:
            for case in args.cases:
                report["warm_summary"][case] = {}
                for name in ("before", "after"):
                    runs = [run for run in report["runs"] if run["case"] == case and run["variant"] == name and run["phase"] == "warm"]
                    report["warm_summary"][case][name] = {
                        "observer": {key: cleanup.summary([run["observer_timing"][key] for run in runs])
                                     for key in (*cleanup.TIMINGS, "lifetime_peak_rss_bytes_at_retention_end")},
                        "events": {key: cleanup.summary([run["events"][key] for run in runs])
                                   for key in ("handler_callback_count", "retained_batch_count", "retained_event_count",
                                               "retained_internal_event_count", "retained_path_utf8_bytes")},
                        "child_cleanup": {key: cleanup.summary([run["child_cleanup_timing"][key] for run in runs]) for key in cleanup.TIMINGS}}
        require(all(cleanup.digest(Path(path)) == value for path, value in source_hashes.items()), "Benchmark sources changed during comparison")
        for key, path in frozen.items():
            require(cleanup.digest(path) == source_hashes[str(originals[key])], "Frozen source copy changed")
        if args.diagnose_history_loss:
            require(all(cleanup.digest(Path(value["instrumented_copy"])) == value["instrumented_sha256"]
                        for value in report["diagnostic_sources"].values()), "Diagnostic service copy changed")
        rust = report["builds"]["rust"]
        require(cleanup.digest(archive) == rust["rlib_sha256"]
                and all(cleanup.digest(Path(path)) == value for path, value in rust["extern_sha256"].items()),
                "A Rust input changed during comparison")
        report["verified"] = True
    except BaseException as error:
        report["error"] = f"{type(error).__name__}: {error}"
        raise
    finally:
        cleanup.write_json(report_path, report)
    print(report_path)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
