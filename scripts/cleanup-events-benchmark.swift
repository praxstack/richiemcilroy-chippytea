// Observe the production FolderWatcher while a generated-only Rust child cleans
// its own fixture. Retention models queued callback payloads, not AppModel or UI.
import Darwin
import Foundation

private struct BenchmarkFailure: Error, CustomStringConvertible {
    let description: String
}

private func require(_ condition: Bool, _ message: String) throws {
    if !condition { throw BenchmarkFailure(description: message) }
}

private func ticks() -> UInt64 { clock_gettime_nsec_np(CLOCK_UPTIME_RAW) }
private let controlBytes = Data("Disposable watcher control; preserve this file.\n".utf8)
private let fixtureMarker = Data("chippytea-cleanup-benchmark-v1\n".utf8)

private func createFile(_ path: URL) throws -> FileHandle {
    let descriptor = open(path.path, O_WRONLY | O_CREAT | O_EXCL | O_CLOEXEC | O_NOFOLLOW_ANY, 0o600)
    try require(descriptor >= 0, "Cannot exclusively create \(path.path): \(errno)")
    return FileHandle(fileDescriptor: descriptor, closeOnDealloc: true)
}

private func physicalDirectory(_ path: URL, device: dev_t? = nil) throws -> stat {
    let descriptor = open(path.path, O_SEARCH | O_CLOEXEC | O_NOFOLLOW_ANY)
    try require(descriptor >= 0, "Expected a physical generated directory: \(path.path)")
    defer { close(descriptor) }
    var value = stat()
    try require(fstat(descriptor, &value) == 0 && value.st_uid == geteuid()
                && value.st_mode & mode_t(S_IFMT) == mode_t(S_IFDIR) && value.st_flags == 0
                && (device == nil || value.st_dev == device), "Generated directory identity/type changed")
    return value
}

private func checkFile(_ path: URL, expected: Data) throws {
    let descriptor = open(path.path, O_RDONLY | O_CLOEXEC | O_NOFOLLOW_ANY | O_NONBLOCK)
    try require(descriptor >= 0, "Cannot open generated control file")
    let file = FileHandle(fileDescriptor: descriptor, closeOnDealloc: true)
    defer { try? file.close() }
    var value = stat()
    try require(fstat(descriptor, &value) == 0 && value.st_uid == geteuid()
                && value.st_mode & mode_t(S_IFMT) == mode_t(S_IFREG) && value.st_nlink == 1
                && value.st_flags == 0 && value.st_size == Int64(expected.count),
                "Generated control file identity/type changed")
    try require(try file.read(upToCount: expected.count + 1) == expected, "Generated control contents changed")
}

private struct Usage {
    let at: UInt64
    let user: Double
    let system: Double
    let rss: Int64

    static func read() throws -> Usage {
        var value = rusage()
        try require(getrusage(RUSAGE_SELF, &value) == 0, "getrusage failed")
        func seconds(_ time: timeval) -> Double { Double(time.tv_sec) + Double(time.tv_usec) / 1_000_000 }
        return Usage(at: ticks(), user: seconds(value.ru_utime), system: seconds(value.ru_stime), rss: Int64(value.ru_maxrss))
    }

    func since(_ before: Usage) -> [String: Any] {
        ["wall_seconds": Double(at - before.at) / 1_000_000_000,
         "user_seconds": user - before.user, "system_seconds": system - before.system,
         "cpu_seconds": user + system - before.user - before.system,
         "lifetime_peak_rss_bytes_at_retention_end": rss]
    }
}

/// Both pipes are drained independently, including while the main thread waits
/// for the child. Only a bounded stderr line buffer is retained in this class.
private final class ChildLogs: @unchecked Sendable {
    let readySignal = DispatchSemaphore(value: 0)
    let drained = DispatchGroup()
    private let lock = NSLock()
    private var ready: ([String: Any], UInt64)?
    private var failure: String?

    func capture(_ pipe: Pipe, into output: FileHandle, stderr: Bool) {
        drained.enter()
        DispatchQueue(label: "chippytea.cleanup-benchmark.\(stderr ? "stderr" : "stdout")").async {
            defer { try? pipe.fileHandleForReading.close(); try? output.close(); self.drained.leave() }
            do {
                var pending = Data()
                var buffer = [UInt8](repeating: 0, count: 16_384)
                while true {
                    // A single POSIX read returns available pipe bytes. Do not
                    // wait for a Foundation read to fill the requested buffer
                    // before delivering the small ready record.
                    let count = buffer.withUnsafeMutableBytes {
                        Darwin.read(pipe.fileHandleForReading.fileDescriptor, $0.baseAddress, $0.count)
                    }
                    if count < 0 && errno == EINTR { continue }
                    try require(count >= 0, "Cannot drain child pipe: \(errno)")
                    if count == 0 { break }
                    let bytes = Data(buffer[..<count])
                    try output.write(contentsOf: bytes)
                    if stderr {
                        pending.append(bytes)
                        while let end = pending.firstIndex(of: 10) {
                            self.line(Data(pending[..<end]))
                            pending.removeSubrange(...end)
                        }
                        try require(pending.count <= 65_536, "Unexpected unbounded child stderr line")
                    }
                }
                if stderr {
                    if !pending.isEmpty { self.line(pending) }
                    self.lock.lock()
                    if self.ready == nil && self.failure == nil { self.failure = "Child exited before its generated fixture was ready" }
                    self.lock.unlock()
                    self.readySignal.signal()
                }
            } catch {
                self.lock.lock()
                self.failure = self.failure ?? String(describing: error)
                self.lock.unlock()
                self.readySignal.signal()
            }
        }
    }

    private func line(_ bytes: Data) {
        guard let value = (try? JSONSerialization.jsonObject(with: bytes)) as? [String: Any],
              value["phase"] as? String == "ready" else { return }
        lock.lock()
        if ready != nil { failure = "Child emitted more than one ready record" }
        else { ready = (value, ticks()) }
        lock.unlock()
        readySignal.signal()
    }

    func readyValue() throws -> ([String: Any], UInt64) {
        lock.lock(); defer { lock.unlock() }
        if let failure { throw BenchmarkFailure(description: failure) }
        guard let ready else { throw BenchmarkFailure(description: "Missing child ready record") }
        return ready
    }

    func check() throws {
        lock.lock(); defer { lock.unlock() }
        if let failure { throw BenchmarkFailure(description: failure) }
    }
}

/// All callback state is protected by the condition. Actual arrays are retained
/// without transforming them into request dictionaries during measurement.
private final class RetainedEvents: @unchecked Sendable {
    private struct Batch {
        let events: [FolderEvent]
        let last: UInt64
        let historyLost: Bool
    }
    private let condition = NSCondition()
    private let control: String
    private let completionControl: String
    private let internalPrefix: String
    private var batches: [Batch] = []
    private var lastCallback: UInt64 = 0
    private var frozen = false

    init(control: String, completionControl: String, internalPrefix: String) {
        self.control = control
        self.completionControl = completionControl
        self.internalPrefix = internalPrefix
    }

    func append(_ events: [FolderEvent], last: UInt64, historyLost: Bool) {
        condition.lock(); defer { condition.unlock() }
        guard !frozen else { return }
        batches.append(Batch(events: events, last: last, historyLost: historyLost))
        lastCallback = ticks()
        condition.signal()
    }

    func finish(after controlWritten: UInt64, deadline: UInt64) throws -> Double {
        condition.lock(); defer { condition.unlock() }
        while true {
            let now = ticks()
            try require(now < deadline, "Watcher did not reach the required quiet tail before its deadline")
            let quietSince = max(controlWritten, lastCallback)
            if now >= quietSince + 1_200_000_000 {
                frozen = true
                return Double(now - quietSince) / 1_000_000_000
            }
            let remaining = min(deadline - now, quietSince + 1_200_000_000 - now)
            _ = condition.wait(until: Date(timeIntervalSinceNow: Double(remaining) / 1_000_000_000))
        }
    }

    // Called after freeze and after the timed endpoint. Keep the arrays alive
    // through that endpoint; dump only generated paths as independent evidence.
    func audit(into path: URL) throws -> [String: Any] {
        condition.lock(); defer { condition.unlock() }
        try require(frozen, "Retention must be frozen before audit")
        let file = try createFile(path)
        defer { try? file.close() }
        var events = 0, pathBytes = 0, internalEvents = 0
        var maxID: UInt64 = 0
        var historyLost = false, controlSeen = false, completionControlSeen = false
        for batch in batches {
            events += batch.events.count
            maxID = max(maxID, batch.last)
            historyLost = historyLost || batch.historyLost
            for event in batch.events {
                pathBytes += event.path.utf8.count
                internalEvents += event.path.hasPrefix(internalPrefix) ? 1 : 0
                controlSeen = controlSeen || (event.path == control && event.kind == "file")
                completionControlSeen = completionControlSeen || (event.path == completionControl && event.kind == "file")
            }
            let value: [String: Any] = ["last": batch.last, "history_lost": batch.historyLost,
                "events": batch.events.map(\.request)]
            try file.write(contentsOf: JSONSerialization.data(withJSONObject: value, options: [.sortedKeys]) + Data([10]))
        }
        try file.synchronize()
        return ["handler_callback_count": batches.count, "retained_batch_count": batches.count,
                "retained_event_count": events, "retained_path_utf8_bytes": pathBytes,
                "retained_internal_event_count": internalEvents, "maximum_event_id": maxID,
                "history_lost": historyLost, "ordinary_control_event_seen": controlSeen,
                "completion_control_event_seen": completionControlSeen]
    }
}

@main
private struct CleanupEventsBenchmark {
    static func main() {
        do {
            let value = try run()
            FileHandle.standardOutput.write(try JSONSerialization.data(withJSONObject: value, options: [.sortedKeys]) + Data([10]))
        } catch {
            fputs("FAIL watcher benchmark: \(error). Generated fixtures and raw evidence are retained.\n", stderr)
            exit(1)
        }
    }

    private static func run() throws -> [String: Any] {
        var options: [String: String] = [:]
        var args = CommandLine.arguments.dropFirst().makeIterator()
        while let key = args.next() {
            guard let value = args.next(), options[key] == nil else { throw BenchmarkFailure(description: "Expected unique option/value pairs") }
            options[key] = value
        }
        try require(Set(options.keys) == Set(["--driver", "--raw-dir", "--case", "--leaves", "--timeout-seconds"]), "Unexpected watcher benchmark options")
        guard let leaves = Int(options["--leaves"]!), (256...131_072).contains(leaves), leaves % 256 == 0,
              let timeout = Double(options["--timeout-seconds"]!), timeout.isFinite, (5...3600).contains(timeout),
              let name = options["--case"], ["regular", "hardlinks"].contains(name) else {
            throw BenchmarkFailure(description: "Invalid workload or timeout")
        }
        let driver = URL(fileURLWithPath: options["--driver"]!)
        let raw = URL(fileURLWithPath: options["--raw-dir"]!, isDirectory: true)
        try require(driver.path.hasPrefix("/") && raw.path.hasPrefix("/"), "Use absolute driver and evidence paths")
        let rawIdentity = try physicalDirectory(raw)
        let rawContents = try FileManager.default.contentsOfDirectory(atPath: raw.path)
        try require(rawIdentity.st_mode & 0o7777 == 0o700 && rawContents.isEmpty,
                    "Raw evidence must use a new owned mode0700 directory")
        let child = Process()
        child.executableURL = driver
        child.arguments = ["--case", name, "--leaves", String(leaves), "--ready-delay-ms", "1000"]
        let stdoutPipe = Pipe(), stderrPipe = Pipe()
        child.standardOutput = stdoutPipe; child.standardError = stderrPipe
        let logs = ChildLogs()
        logs.capture(stdoutPipe, into: try createFile(raw.appendingPathComponent("child.stdout.json")), stderr: false)
        logs.capture(stderrPipe, into: try createFile(raw.appendingPathComponent("child.stderr.log")), stderr: true)
        let exited = DispatchSemaphore(value: 0)
        child.terminationHandler = { _ in exited.signal() }
        defer {
            try? stdoutPipe.fileHandleForWriting.close(); try? stderrPipe.fileHandleForWriting.close()
            if child.isRunning {
                child.terminate()
                if exited.wait(timeout: .now() + 2) == .timedOut, child.isRunning {
                    _ = kill(child.processIdentifier, SIGKILL)
                    _ = exited.wait(timeout: .now() + 2)
                }
            }
        }
        let deadline = ticks() + UInt64(timeout * 1_000_000_000)
        let dispatchDeadline = DispatchTime.now() + timeout
        try child.run()
        try stdoutPipe.fileHandleForWriting.close(); try stderrPipe.fileHandleForWriting.close()
        try require(logs.readySignal.wait(timeout: dispatchDeadline) == .success, "Timed out waiting for generated fixture readiness")
        let (ready, received) = try logs.readyValue()
        guard let fixturePath = ready["fixture"] as? String, let pid = ready["pid"] as? Int,
              pid == Int(child.processIdentifier), ready["case"] as? String == name,
              ready["leaves"] as? Int == leaves, ready["delay_ms"] as? Int == 1000 else {
            throw BenchmarkFailure(description: "Invalid child ready record")
        }
        let fixture = URL(fileURLWithPath: fixturePath, isDirectory: true)
        try require(fixture.deletingLastPathComponent().path == "/private/tmp"
                    && fixture.lastPathComponent.range(of: "^chippytea-cleanup-[0-9a-f]{32}$", options: .regularExpression) != nil,
                    "Refusing to watch anything except the child's generated fixture")
        let identity = try physicalDirectory(fixture)
        try require(identity.st_mode & 0o7777 == 0o700, "Generated fixture must be private")
        try checkFile(fixture.appendingPathComponent(".chippytea-cleanup-fixture"), expected: fixtureMarker)
        let projects = fixture.appendingPathComponent("Projects")
        _ = try physicalDirectory(projects, device: identity.st_dev)
        let sibling = projects.appendingPathComponent("Sibling")
        _ = try physicalDirectory(sibling, device: identity.st_dev)
        let control = sibling.appendingPathComponent("watcher-control.txt")
        let completionControl = sibling.appendingPathComponent("watcher-complete-control.txt")
        let retained = RetainedEvents(control: control.path, completionControl: completionControl.path,
                                      internalPrefix: projects.appendingPathComponent("Disposable/.chippytea-").path)
        let start = try Usage.read()
        var watcher: FolderWatcher? = FolderWatcher(paths: [projects.path], since: 0) { events, last, lost in
            retained.append(events, last: last, historyLost: lost)
        }
        try require(watcher?.isRunning == true, "Production FolderWatcher did not start")
        let attached = ticks()
        let attachmentMilliseconds = Double(attached - received) / 1_000_000
        try require(attachmentMilliseconds < 800, "Watcher attached too late for the child's fixed delay")
        _ = try physicalDirectory(projects.appendingPathComponent("Disposable/target"), device: identity.st_dev)
        let controlFile = try createFile(control)
        try controlFile.write(contentsOf: controlBytes); try controlFile.synchronize(); try controlFile.close()
        let (quietSeconds, completionControlDelay) = try withExtendedLifetime(watcher) {
            try require(exited.wait(timeout: dispatchDeadline) == .success, "Generated cleanup child timed out")
            let childFinished = ticks()
            try require(logs.drained.wait(timeout: dispatchDeadline) == .success, "Child pipes did not drain")
            try logs.check()
            try require(child.terminationReason == .exit && child.terminationStatus == 0, "Generated cleanup child failed; inspect retained logs")
            let resultData = try Data(contentsOf: raw.appendingPathComponent("child.stdout.json"))
            guard let result = try JSONSerialization.jsonObject(with: resultData) as? [String: Any],
                  result["protocol"] as? Int == 1, result["verified"] as? Bool == true,
                  result["fixture"] as? String == fixture.path, result["case"] as? String == name,
                  result["leaves"] as? Int == leaves, result["artifact_absent"] as? Bool == true,
                  let receipt = result["receipt"] as? [String: Any], receipt["outcome"] as? String == "removed" else {
                throw BenchmarkFailure(description: "Child did not produce a completed verified cleanup result")
            }
            // This distinct path cannot have appeared in the initial control
            // batch. Its event proves the watcher still receives ordinary work
            // after the child has exited and its cleanup result is complete.
            let completionFile = try createFile(completionControl)
            try completionFile.write(contentsOf: controlBytes)
            try completionFile.synchronize(); try completionFile.close()
            let written = ticks()
            let quiet = try retained.finish(after: written, deadline: min(deadline, childFinished + 15_000_000_000))
            return (quiet, Double(written - childFinished) / 1_000_000)
        }
        watcher = nil
        let end = try Usage.read()
        let events = try retained.audit(into: raw.appendingPathComponent("events.jsonl"))
        try require(events["history_lost"] as? Bool == false, "Watcher reported event history loss")
        try require(events["ordinary_control_event_seen"] as? Bool == true, "Watcher missed the initial ordinary control")
        try require(events["completion_control_event_seen"] as? Bool == true, "Watcher missed the post-cleanup ordinary control")
        try require((events["maximum_event_id"] as? UInt64 ?? 0) > 0, "Watcher did not deliver an event cursor")
        try checkFile(control, expected: controlBytes)
        try checkFile(completionControl, expected: controlBytes)
        return ["protocol": 1, "verified": true, "case": name, "leaves": leaves,
                "fixture": fixture.path, "raw_directory": raw.path, "rust_driver": driver.path,
                "child_exit_status": child.terminationStatus, "watcher_attach_ms": attachmentMilliseconds,
                "artifact_present_when_watcher_started": true, "quiet_tail_seconds": quietSeconds,
                "quiet_tail_limit_seconds": 15,
                "control_path": control.path, "completion_control_path": completionControl.path,
                "child_result_checked_before_completion_control": true,
                "completion_control_after_child_exit_ms": completionControlDelay,
                "events": events, "observer_timing": end.since(start),
                "scope": "Production FolderWatcher plus retained handler batches on the generated child's Projects root. Observer timing starts before watcher construction and ends after child completion, a second ordinary control, a 1.2s quiet tail, and watcher teardown. It includes both control creations, pipe draining, completed-child result checks and waiting for the child's post-cleanup audits. Event serialization, final observer audits and fixture generation are excluded. RUSAGE_SELF excludes the Rust child and diskutil CPU. RSS is observer lifetime peak at retention end, including setup, not Rust memory. This does not run AppModel, the engine request queue or UI. FSEvents may coalesce; callback counts are delivered handler batches, not raw OS callbacks."]
    }
}
