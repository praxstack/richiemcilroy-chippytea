import Foundation
import SwiftUI

struct CleanupProgress: Decodable, Equatable, Sendable {
    let phase: String
    let completedEntries: UInt64
    let totalEntries: UInt64
    let itemNumber: Int
    let itemCount: Int
    let title: String

    var headline: String {
        switch phase {
        case "checking": return "Checking reviewed files…"
        case "preparing": return "Preparing cleanup…"
        case "comparing": return "Verifying both copies…"
        case "removing": return "Removing files…"
        case "accounting": return "Checking recovered space…"
        default: return "Starting cleanup…"
        }
    }

    var detail: String {
        if phase == "comparing" {
            return ByteCountFormatter.string(fromByteCount: Int64(clamping: completedEntries), countStyle: .file)
                + " compared · your kept copy stays in place"
        }
        if totalEntries > 0 {
            return "\(min(completedEntries, totalEntries).formatted()) of \(totalEntries.formatted()) entries"
        }
        if completedEntries > 0 { return "\(completedEntries.formatted()) entries checked" }
        return itemCount > 1 ? "Item \(itemNumber) of \(itemCount)" : title
    }
}

struct EngineIdentity: Codable, Hashable {
    var device: UInt64
    var inode: UInt64
    var mode: UInt32
    var size: UInt64
    var modifiedNs: Int64
    var changedNs: Int64
}

struct ScanRoot: Codable, Identifiable, Equatable {
    var id: String
    var path: String
    var kind: String
    var identity: EngineIdentity
    var name: String { URL(fileURLWithPath: path).lastPathComponent }
}

struct RootAccessIssue: Decodable, Identifiable, Equatable {
    let rootId: String
    let path: String
    let status: String
    let message: String?
    var id: String { rootId }
    var needsAttention: Bool { status != "available" }
}

struct Candidate: Codable, Identifiable, Hashable {
    var id: String
    var rootId: String
    var path: String
    var title: String
    var kind: String
    var logicalBytes: UInt64
    var allocatedBytes: UInt64
    var fileCount: UInt64
    var modifiedNs: Int64
    var explanation: String
    var consequence: String
    var eligiblePermanent: Bool
    var blockedReason: String?
    var identity: EngineIdentity
    var fingerprint: String
    var evidence: String
    /// The engine's own recommendation policy. Absent from legacy indexes, where the
    /// conservative reading is "not recommended"; it never authorizes anything by itself.
    var suggestionEligible: Bool?
    var recommended: Bool { suggestionEligible == true && canReviewCleanup }
    var isDeveloper: Bool {
        switch kind {
        case "node", "cargo", "venv", "webcache", "devcache", "pythoncache", "xcode", "swiftpm", "dotnet", "gradle", "dart", "flutter", "zig": return true
        default: return false
        }
    }
    var isCacheOrLog: Bool {
        switch kind {
        case "devcache", "cache", "log", "crashreport": return true
        default: return false
        }
    }
    var isPersonalFile: Bool {
        switch kind {
        case "download", "installer", "archive", "largefile": return true
        default: return false
        }
    }
    var cleanupBlockedReason: String? {
        if let blockedReason { return blockedReason }
        return isDeveloper || isCacheOrLog || isPersonalFile
            ? nil : "This item type is not supported by this version of chippytea."
    }
    /// Early activity exclusions deliberately skip cache traversal. An unknown
    /// allocation must not be presented as an empty cache or recoverable space.
    var measuredAllocatedBytes: UInt64? {
        if kind == "devcache", blockedReason != nil, fingerprint.isEmpty,
           fileCount == 0, allocatedBytes == 0 { return nil }
        return allocatedBytes
    }
    var canReviewCleanup: Bool { cleanupBlockedReason == nil }
    /// A category is not permission to delete: only the existing verified
    /// developer kinds can opt into permanent cleanup. New kinds fail closed.
    var canDeletePermanently: Bool {
        guard canReviewCleanup, eligiblePermanent else { return false }
        switch kind {
        case "node", "cargo", "venv", "webcache", "devcache": return true
        default: return false
        }
    }
    var symbol: String {
        switch kind {
        case "node": return "shippingbox"
        case "cargo": return "hammer"
        case "venv": return "terminal"
        case "webcache": return "arrow.triangle.2.circlepath"
        case "devcache": return "arrow.triangle.2.circlepath"
        case "pythoncache": return "terminal"
        case "cache": return "externaldrive"
        case "log": return "doc.text"
        case "crashreport": return "exclamationmark.bubble"
        case "xcode": return "hammer"
        case "swiftpm", "dotnet", "gradle", "flutter", "zig": return "hammer"
        case "dart": return "wrench.and.screwdriver"
        case "installer": return "shippingbox"
        case "archive": return "doc.zipper"
        case "largefile": return "doc"
        case "download": return "arrow.down.doc"
        default: return "questionmark.folder"
        }
    }
    var category: String {
        switch kind {
        case "node": return "Dependencies"
        case "cargo": return "Build artifacts"
        case "venv": return "Python environment"
        case "webcache": return "Build cache"
        case "devcache": return "Developer cache"
        case "pythoncache": return "Python bytecode cache"
        case "cache": return "App cache"
        case "log": return "App log"
        case "crashreport": return "Crash report"
        case "xcode": return "Xcode build data"
        case "swiftpm": return "SwiftPM build data"
        case "dotnet": return ".NET build data"
        case "gradle": return "Gradle build data"
        case "dart": return "Dart tool data"
        case "flutter": return "Flutter build data"
        case "zig": return "Zig build cache"
        case "installer": return "Installer"
        case "archive": return "Archive"
        case "largefile": return "Large personal file"
        case "download": return "Download"
        default: return "Review item"
        }
    }
    /// Display only: preserve the engine's title and exact path for cleanup review.
    /// Lexical path components need no filesystem access or ownership lookup.
    var displayName: String {
        let location = path as NSString
        let name = location.lastPathComponent
        guard !name.isEmpty else { return title }
        switch kind {
        case "devcache": return title
        case "node", "cargo", "venv", "webcache", "pythoncache", "swiftpm", "dotnet", "gradle", "dart", "flutter", "zig":
            let parent = (location.deletingLastPathComponent as NSString).lastPathComponent
            return parent.isEmpty || parent == "/" ? name : "\(parent) / \(name)"
        default:
            return name
        }
    }

    func matchesSearch(_ query: String) -> Bool {
        query.isEmpty || path.localizedCaseInsensitiveContains(query)
            || title.localizedCaseInsensitiveContains(query)
            || displayName.localizedCaseInsensitiveContains(query)
    }
}

enum DiscoveryFilter: String, CaseIterable {
    case all = "All", caches = "Caches & logs", developer = "Developer", personal = "Personal files"

    func matches(_ candidate: Candidate) -> Bool {
        switch self {
        case .all: return true
        case .caches: return candidate.isCacheOrLog
        case .developer: return candidate.isDeveloper
        case .personal: return candidate.isPersonalFile
        }
    }
}

enum DiscoverySort: String, CaseIterable {
    case suggested = "Suggested first", largest = "Largest first", oldest = "Oldest first", name = "Name"

    func ordered(_ candidates: [Candidate]) -> [Candidate] {
        // The engine has already ranked recommendations. Filtering retains
        // that order instead of replacing it with a size-only sort.
        switch self {
        case .suggested:
            return candidates
        case .largest:
            return candidates.sorted { lhs, rhs in
                lhs.allocatedBytes == rhs.allocatedBytes ? lhs.path < rhs.path : lhs.allocatedBytes > rhs.allocatedBytes
            }
        case .oldest:
            return candidates.sorted { lhs, rhs in
                lhs.modifiedNs == rhs.modifiedNs ? lhs.path < rhs.path : lhs.modifiedNs < rhs.modifiedNs
            }
        case .name:
            // Derive each visible name once, not during every comparison.
            return candidates.map { (candidate: $0, name: $0.displayName) }.sorted { lhs, rhs in
                let order = lhs.name.localizedStandardCompare(rhs.name)
                return order == .orderedSame ? lhs.candidate.path < rhs.candidate.path : order == .orderedAscending
            }.map { $0.candidate }
        }
    }
}

struct ScanStats: Codable, Equatable {
    var entries: UInt64 = 0
    var files: UInt64 = 0
    var directories: UInt64 = 0
    var logicalBytes: UInt64 = 0
    var allocatedBytes: UInt64 = 0
    var skipped: UInt64 = 0
    var excludedArtifacts: UInt64 = 0
    var errors: UInt64 = 0
    var candidates: UInt64 = 0
    var elapsedMs: UInt64 = 0
    var firstFindingMs: UInt64?
    var cancelled = false
    var complete = false
    var message = "Choose a folder to start discovering."
}

struct ForegroundScan: Codable, Equatable {
    var active: Bool
    var stats: ScanStats
}

struct Wallet: Codable, Equatable {
    var collectedCoins: UInt64 = 0
    var pendingCoins: UInt64 = 0
    var fractionalBytes: UInt64 = 0
    var creditedBytes: UInt64 = 0
}

struct Receipt: Codable, Identifiable, Equatable {
    var id: String
    var path: String
    var title: String
    var operation: String
    var outcome: String
    var detail: String
    var createdAt: Int64
    var reportedBytes: UInt64
    var observedBytes: UInt64
    var creditedBytes: UInt64
    var coins: UInt64
    var trashPath: String?
    var canRestore: Bool
    /// Ledger position, present only on paged history responses.
    var seq: Int64? = nil
}

struct EngineSnapshot: Codable, Equatable {
    var roots: [ScanRoot] = []
    var candidates: [Candidate] = []
    var history: [Receipt] = []
    var wallet = Wallet()
    var scanning = false
    var cleaning = false
    var stats = ScanStats()
    var foregroundScan: ForegroundScan?
    var error: String?
    var keptPaths: [String] = []

    /// Raw counters describe engine work, not the optional foreground result.
    /// Keep control flags and every review/reward field visible immediately.
    func hasSamePresentation(as other: EngineSnapshot) -> Bool {
        scanning == other.scanning && cleaning == other.cleaning
            && foregroundScan == other.foregroundScan && error == other.error
            && roots == other.roots && candidates == other.candidates
            && history == other.history && wallet == other.wallet && keptPaths == other.keptPaths
    }
}

struct CollectionBurst: Identifiable {
    let id = UUID()
    let from: UInt64
    let to: UInt64
    let amount: UInt64
    var showsParticles = true
}

/// Consent captures these exact candidates. Prepare runs only at the queue head,
/// so a later filesystem change must fail validation rather than change the job.
struct CleanupRequest {
    let id = UUID()
    let items: [Candidate]
    let permanently: Bool
    let duplicate: DuplicateCleanupChoice?

    init(items: [Candidate], permanently: Bool, duplicate: DuplicateCleanupChoice? = nil) {
        self.items = items
        self.permanently = permanently
        self.duplicate = duplicate
    }
}

struct DuplicateCleanupChoice {
    let reportToken: String
    let groupID: String
    let keeperID: String
    let copyID: String
    let keeper: Candidate

    var prepareRequest: [String: Any] {
        ["action": "prepare_duplicate", "operation": "trash", "report_token": reportToken,
         "group_id": groupID, "keeper_id": keeperID, "copy_id": copyID]
    }
}

/// One projection across all outstanding jobs applies the wallet's fractional
/// remainder once. Trash contributes to work estimates, never to coin estimates.
struct CleanupEstimate: Equatable {
    let confirmedCoins: UInt64
    let targetCoins: UInt64
    let pendingCoins: UInt64
    let allocatedBytes: UInt64
    let itemCount: Int
    let hasPermanentCleanup: Bool

    init(requests: [CleanupRequest], wallet: Wallet, collectedCoinsFloor: UInt64 = 0) {
        func adding(_ lhs: UInt64, _ rhs: UInt64) -> UInt64 {
            let sum = lhs.addingReportingOverflow(rhs)
            return sum.overflow ? .max : sum.partialValue
        }
        confirmedCoins = max(collectedCoinsFloor, adding(wallet.collectedCoins, wallet.pendingCoins))
        var totalBytes: UInt64 = 0
        var eligibleBytes: UInt64 = 0
        var count = 0
        var permanent = false
        for request in requests {
            count += request.items.count
            permanent = permanent || request.permanently
            let eligible = request.permanently && !request.items.isEmpty
                && request.items.allSatisfy(\.canDeletePermanently)
            for item in request.items {
                totalBytes = adding(totalBytes, item.allocatedBytes)
                if eligible { eligibleBytes = adding(eligibleBytes, item.allocatedBytes) }
            }
        }
        allocatedBytes = totalBytes
        itemCount = count
        hasPermanentCleanup = permanent
        let coinBytes: UInt64 = 100_000_000
        let remainder = wallet.fractionalBytes < coinBytes ? wallet.fractionalBytes : 0
        pendingCoins = eligibleBytes / coinBytes + (eligibleBytes % coinBytes + remainder) / coinBytes
        targetCoins = adding(confirmedCoins, pendingCoins)
    }
}

/// A transient presentation for a confirmed addition to the queue; it never
/// awards coins. Only Rust's committed ledger survives restart.
struct CleanupPreview: Identifiable, Equatable {
    let id = UUID()
    let from: UInt64
    let estimatedTo: UInt64
    let estimatedCoins: UInt64
    let estimatedBytes: UInt64
    let itemCount: Int
    let isPermanent: Bool
    var adjustedTo: UInt64?
    var resolvedTo: UInt64?
    var presentationFinished = false

    var targetCoins: UInt64 { resolvedTo ?? adjustedTo ?? estimatedTo }

    init(items: [Candidate], wallet: Wallet, permanently: Bool, collectedCoinsFloor: UInt64 = 0) {
        self.init(estimate: CleanupEstimate(requests: [CleanupRequest(items: items, permanently: permanently)],
                                           wallet: wallet, collectedCoinsFloor: collectedCoinsFloor),
                  permanently: permanently)
    }

    init(estimate: CleanupEstimate, from: UInt64? = nil, permanently: Bool) {
        self.from = from ?? estimate.confirmedCoins
        itemCount = estimate.itemCount
        isPermanent = permanently
        estimatedBytes = estimate.allocatedBytes
        estimatedCoins = estimate.pendingCoins
        estimatedTo = estimate.targetCoins
    }
}

enum Destination: String, CaseIterable, Identifiable {
    case coins = "Your chips", discover = "Find space", activity = "Activity", settings = "Settings"
    var id: String { rawValue }
    /// `.coins` draws the chip doodle itself rather than a system glyph.
    var symbol: String {
        switch self { case .coins: return "circle"; case .discover: return "magnifyingglass"; case .activity: return "clock.arrow.circlepath"; case .settings: return "gearshape" }
    }
    /// The compact label used by the panel's bottom bar; `rawValue` stays the accessible name.
    var tabTitle: String {
        switch self { case .coins: return "chippytea"; case .discover: return "Find space"; case .activity: return "Activity"; case .settings: return "Settings" }
    }
}

// MARK: - Chips

/// The wallet and engine count plain chips; this is how the interface says them.
func chipsPhrase(_ total: UInt64) -> String {
    "\(total.formatted()) \(total == 1 ? "chip" : "chips")"
}

func space(_ bytes: UInt64) -> String {
    let value = Double(bytes)
    if bytes >= 1_000_000_000 { return String(format: "%.2f GB", value / 1_000_000_000) }
    if bytes >= 1_000_000 { return String(format: "%.1f MB", value / 1_000_000) }
    if bytes >= 1_000 { return String(format: "%.0f KB", value / 1_000) }
    return "\(bytes) B"
}
