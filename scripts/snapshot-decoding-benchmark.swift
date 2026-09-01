import CryptoKit
import Darwin
import Foundation

private struct BenchmarkFailure: Error, CustomStringConvertible {
    let description: String
}

private func require(_ condition: Bool, _ message: String) throws {
    if !condition { throw BenchmarkFailure(description: message) }
}

private struct Envelope: Encodable {
    let ok = true
    let data: EngineSnapshot
}

private func snapshot(candidateCount: Int, changed: Bool, oversized: Bool) -> EngineSnapshot {
    let precise: UInt64 = 9_007_199_254_740_993
    let identity = EngineIdentity(device: precise, inode: UInt64.max - 20, mode: 0o040700,
        size: 104_857_600, modifiedNs: 1_700_000_000_000_000_001, changedNs: 1_700_000_000_000_000_003)
    var value = EngineSnapshot()
    value.roots = [ScanRoot(id: "synthetic-root", path: "/synthetic/chippytea-decoder", kind: "projects", identity: identity)]
    value.candidates = (0..<candidateCount).map { index in
        var fileIdentity = identity
        fileIdentity.inode -= UInt64(index + 1)
        return Candidate(id: "synthetic-\(index)", rootId: "synthetic-root",
            path: "/synthetic/chippytea-decoder/project-\(index)/target", title: "Synthetic build output \(index)",
            kind: "cargo", logicalBytes: precise + UInt64(index), allocatedBytes: 104_857_600,
            fileCount: 8194, modifiedNs: identity.modifiedNs,
            explanation: String(repeating: "Synthetic ownership evidence. ", count: 4),
            consequence: "Synthetic consequence; this benchmark never opens an engine or cleans files.",
            eligiblePermanent: true, blockedReason: nil, identity: fileIdentity,
            fingerprint: String(repeating: "a", count: 64), evidence: String(repeating: "b", count: 64),
            suggestionEligible: true)
    }
    value.wallet = Wallet(collectedCoins: precise, pendingCoins: precise + (changed ? 1 : 0),
        fractionalBytes: 50_000_000, creditedBytes: UInt64.max - 10)
    value.stats.entries = precise + (changed ? 1 : 0)
    value.stats.files = 8194
    value.stats.directories = 3
    value.stats.candidates = UInt64(candidateCount)
    value.stats.complete = true
    value.stats.message = oversized ? String(repeating: "x", count: 1024 * 1024) : "Synthetic completed scan"
    var foregroundStats = value.stats
    foregroundStats.message = "Synthetic completed scan"
    value.foregroundScan = ForegroundScan(active: false, stats: foregroundStats)
    value.history = [Receipt(id: "synthetic-receipt", path: "/synthetic/chippytea-decoder/previous/target",
        title: "Synthetic history", operation: "permanent", outcome: "completed", detail: "No real cleanup occurred.",
        createdAt: 1_700_000_000, reportedBytes: precise, observedBytes: precise,
        creditedBytes: precise, coins: precise, trashPath: nil, canRestore: false)]
    value.keptPaths = ["/synthetic/chippytea-decoder/kept"]
    return value
}

@inline(never)
private func freshCopy(_ data: Data) -> Data {
    data.withUnsafeBytes { bytes in Data(bytes: bytes.baseAddress!, count: bytes.count) }
}

private func usage() throws -> rusage {
    var value = rusage()
    try require(getrusage(RUSAGE_SELF, &value) == 0, "getrusage failed")
    return value
}

private func seconds(_ value: timeval) -> Double {
    Double(value.tv_sec) + Double(value.tv_usec) / 1_000_000
}

@main
private struct SnapshotDecodingBenchmark {
    static func main() {
        do { try run() }
        catch {
            let record: [String: Any] = ["protocol": 1, "verified": false, "failure": String(describing: error)]
            if let data = try? JSONSerialization.data(withJSONObject: record, options: [.sortedKeys]) {
                FileHandle.standardOutput.write(data + Data([10]))
            }
            exit(1)
        }
    }

    private static func run() throws {
        let arguments = CommandLine.arguments
        try require(arguments.count == 3, "Expected case and iteration count")
        let name = arguments[1]
        let allowed = ["nine-identical", "fivehundred-identical", "nine-changing", "fivehundred-changing", "oversized-identical"]
        guard let iterations = Int(arguments[2]), (2...10_000).contains(iterations), allowed.contains(name) else {
            throw BenchmarkFailure(description: "Invalid case or iteration count")
        }
        let count = name.hasPrefix("fivehundred") ? 500 : 9
        let changing = name.hasSuffix("changing")
        let oversized = name == "oversized-identical"
        let expected = [snapshot(candidateCount: count, changed: false, oversized: oversized),
                        snapshot(candidateCount: count, changed: changing, oversized: oversized)]
        let encoder = JSONEncoder()
        encoder.keyEncodingStrategy = .convertToSnakeCase
        encoder.outputFormatting = [.sortedKeys, .withoutEscapingSlashes]
        let wires = try expected.map { try encoder.encode(Envelope(data: $0)) }
        try require(wires[0].count == wires[1].count, "Changed-response control must retain wire length")
        try require((wires[0] != wires[1]) == changing, "Unexpected response equivalence")
        try require(oversized ? wires[0].count > 1024 * 1024 : wires[0].count <= 1024 * 1024,
                    "Synthetic response crossed its declared cache boundary")
        #if SNAPSHOT_CACHE
        try require(SnapshotResponseDecoder.maximumResponseBytes == 1024 * 1024, "Production cache cap changed")
        var decoder = SnapshotResponseDecoder()
        let implementation = "candidate"
        #else
        let implementation = "baseline"
        #endif

        var checksum: UInt64 = 0
        let before = try usage()
        let started = clock_gettime_nsec_np(CLOCK_UPTIME_RAW)
        for iteration in 0..<iterations {
            // Force a new allocation, matching the bridge's copy of fresh FFI
            // bytes. No engine, queue, pointer response or filesystem is used.
            let index = iteration % 2
            let input = freshCopy(wires[index])
            #if SNAPSHOT_CACHE
            let decoded = try decoder.decode(input)
            #else
            let decoded = try EngineClient.decodeSnapshotResponse(input)
            #endif
            try require(decoded == expected[index], "Decoded fields or integer precision changed")
            checksum &+= decoded.stats.entries
            checksum &+= decoded.wallet.pendingCoins
            checksum &+= decoded.candidates[count - 1].identity.inode
        }
        let finished = clock_gettime_nsec_np(CLOCK_UPTIME_RAW)
        let after = try usage()
        var expectedChecksum: UInt64 = 0
        for iteration in 0..<iterations {
            let value = expected[iteration % 2]
            expectedChecksum &+= value.stats.entries
            expectedChecksum &+= value.wallet.pendingCoins
            expectedChecksum &+= value.candidates[count - 1].identity.inode
        }
        try require(checksum == expectedChecksum, "Observable checksum changed")
        let record: [String: Any] = [
            "protocol": 1, "verified": true, "variant": implementation, "case": name,
            "iterations": iterations, "candidate_count": count, "changing": changing, "oversized": oversized,
            "wire_bytes": wires.map(\.count),
            "wire_sha256": wires.map { SHA256.hash(data: $0).map { String(format: "%02x", $0) }.joined() },
            "checksum": String(checksum), "precision_checked": true, "cache_starts_empty": true,
            "loop_wall_ms": Double(finished - started) / 1_000_000,
            "loop_user_cpu_seconds": seconds(after.ru_utime) - seconds(before.ru_utime),
            "loop_system_cpu_seconds": seconds(after.ru_stime) - seconds(before.ru_stime),
            "peak_process_rss_bytes_at_loop_end": Int64(after.ru_maxrss),
            "timing_scope": "Fresh Data copy, response decoding/reuse, and full equality/precision validation per iteration. Setup, encoding, hashing, output and process startup excluded. CPU is getrusage user+system; wall is CLOCK_UPTIME_RAW. RSS is lifetime process peak, including setup. No FFI, scanner, UI, library or filesystem operations."
        ]
        let result = try JSONSerialization.data(withJSONObject: record, options: [.sortedKeys])
        FileHandle.standardOutput.write(result + Data([10]))
    }
}
