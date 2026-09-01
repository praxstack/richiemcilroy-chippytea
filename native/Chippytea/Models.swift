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
        case "removing": return "Removing files…"
        case "accounting": return "Checking recovered space…"
        default: return "Starting cleanup…"
        }
    }

    var detail: String {
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
    var recommended: Bool { suggestionEligible == true && blockedReason == nil }
    /// Every kind except a personal download is a recognised developer artifact.
    var isDeveloper: Bool { kind != "download" }
    var symbol: String {
        switch kind {
        case "node": return "shippingbox"
        case "cargo": return "hammer"
        case "venv": return "terminal"
        case "webcache": return "arrow.triangle.2.circlepath"
        default: return "arrow.down.doc"
        }
    }
    var category: String {
        switch kind {
        case "node": return "Dependencies"
        case "cargo": return "Build artifacts"
        case "venv": return "Python environment"
        case "webcache": return "Build cache"
        default: return "Review a download"
        }
    }
    var project: String { URL(fileURLWithPath: path).deletingLastPathComponent().lastPathComponent }
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
                && request.items.allSatisfy { $0.eligiblePermanent && $0.blockedReason == nil }
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
        switch self { case .coins: return "Chippytea"; case .discover: return "Find space"; case .activity: return "Activity"; case .settings: return "Settings" }
    }
}

// MARK: - Fish and chips

/// The chip-shop denominations. The wallet and engine count plain chips;
/// a battered fish is the interface's name for every full thousand.
let chipsPerFish: UInt64 = 1000

struct FishAndChips: Equatable {
    let fish: UInt64
    let chips: UInt64
    init(totalChips: UInt64) {
        fish = totalChips / chipsPerFish
        chips = totalChips % chipsPerFish
    }
}

/// The hero phrasing: both denominations, always — "0 fish, 900 chips".
func fishAndChipsPhrase(_ total: UInt64) -> String {
    let order = FishAndChips(totalChips: total)
    return "\(order.fish.formatted()) fish, \(order.chips.formatted()) \(order.chips == 1 ? "chip" : "chips")"
}

/// Compact phrasing for captions and receipts: plain chips below one fish,
/// both denominations from the first full fish.
func chipsPhrase(_ total: UInt64) -> String {
    guard total >= chipsPerFish else { return "\(total.formatted()) \(total == 1 ? "chip" : "chips")" }
    return fishAndChipsPhrase(total)
}

func space(_ bytes: UInt64) -> String {
    let value = Double(bytes)
    if bytes >= 1_000_000_000 { return String(format: "%.2f GB", value / 1_000_000_000) }
    if bytes >= 1_000_000 { return String(format: "%.1f MB", value / 1_000_000) }
    if bytes >= 1_000 { return String(format: "%.0f KB", value / 1_000) }
    return "\(bytes) B"
}
