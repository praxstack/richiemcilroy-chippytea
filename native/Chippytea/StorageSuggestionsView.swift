import Foundation
import SwiftUI

/// Automatic filesystem observations stay outside CandidateDTO and cleanup.
/// Unknown authority or state values fail decoding rather than becoming actions.
struct StorageInventoryReportDTO: Decodable {
    enum State: String, Decodable {
        case complete = "Complete"
        case partial = "Partial"
        case unavailable = "Unavailable"
    }

    enum CleanupAuthority: String, Decodable {
        case reviewOnly = "ReviewOnly"
    }

    enum IssueKind: String, Decodable {
        case permissionDenied = "PermissionDenied"
        case symlink = "Symlink"
        case changed = "Changed"
        case depthLimit = "DepthLimit"
        case entryLimit = "EntryLimit"
        case timeLimit = "TimeLimit"
        case cloudPlaceholder = "CloudPlaceholder"
        case mountBoundary = "MountBoundary"
        case unavailable = "Unavailable"
    }

    struct Row: Decodable, Identifiable {
        let id: String
        let title: String
        let category: String
        let path: String
        let state: State
        let allocatedBytes: UInt64?
        let logicalBytes: UInt64?
        let files: UInt64
        let directories: UInt64
        let detail: String
        let ownerFollowup: String
        let provider: String?
        let cleanupAuthority: CleanupAuthority

        var owner: ManagedProviderID? {
            guard let provider else { return nil }
            return ManagedProviderID(rawValue: provider)
        }

        var sizeLabel: String {
            guard let allocatedBytes else { return "Size unavailable" }
            let size = space(allocatedBytes)
            return state == .partial ? size + " measured" : size
        }

        func matches(filter: DiscoveryFilter, query: String) -> Bool {
            let categoryMatches: Bool
            switch filter {
            case .all: categoryMatches = true
            case .developer: categoryMatches = category == "Developer tools" || category == "Containers"
            case .caches:
                categoryMatches = ["cache", "logs", "reports"].contains { title.localizedCaseInsensitiveContains($0) }
            case .personal: categoryMatches = false
            }
            return categoryMatches && (query.isEmpty || title.localizedCaseInsensitiveContains(query)
                || path.localizedCaseInsensitiveContains(query) || category.localizedCaseInsensitiveContains(query))
        }
    }

    struct Issue: Decodable {
        let path: String
        let kind: IssueKind
        let detail: String
    }

    let rows: [Row]
    let issues: [Issue]
    let omittedIssues: UInt64
    let examinedEntries: UInt64
    let elapsedMs: UInt64
    let complete: Bool

    static func decode(_ data: Data) throws -> Self {
        let report = try EngineClient.decode(Self.self, data)
        guard report.rows.count <= 64, report.issues.count <= 64,
              Set(report.rows.map(\.id)).count == report.rows.count,
              !report.complete || (report.issues.isEmpty && report.omittedIssues == 0),
              report.rows.allSatisfy({ row in
                  row.cleanupAuthority == .reviewOnly && row.path.hasPrefix("/") &&
                  !row.id.isEmpty && !row.title.isEmpty &&
                  (row.provider == nil || ["homebrew", "uv", "docker"].contains(row.provider!)) &&
                  (row.allocatedBytes == nil) == (row.logicalBytes == nil) &&
                  (row.state != .complete || row.allocatedBytes != nil) &&
                  (row.state != .unavailable || row.allocatedBytes == nil) &&
                  (!report.complete || row.state == .complete)
              }) else {
            throw InventoryDecodingError.invalidResponse
        }
        return report
    }

    private enum InventoryDecodingError: LocalizedError {
        case invalidResponse
        var errorDescription: String? {
            "The storage overview returned an unexpected response. Scan again to refresh it."
        }
    }
}

/// One list combines engine-authorized cleanup with tool-owned observations.
/// Observations never acquire a Candidate identity or enter cleanup selection.
enum DiscoveryFinding: Identifiable {
    case candidate(Candidate)
    case managed(StorageInventoryReportDTO.Row)

    var id: String {
        switch self {
        case .candidate(let item): return "candidate:" + item.id
        case .managed(let item): return "managed:" + item.id
        }
    }
    var path: String {
        switch self {
        case .candidate(let item): return item.path
        case .managed(let item): return item.path
        }
    }
    var title: String {
        switch self {
        case .candidate(let item): return item.displayName
        case .managed(let item): return item.title
        }
    }
    var allocatedBytes: UInt64? {
        switch self {
        case .candidate(let item): return item.measuredAllocatedBytes
        case .managed(let item): return item.allocatedBytes
        }
    }
    var modifiedNs: Int64? {
        switch self {
        case .candidate(let item): return item.modifiedNs
        case .managed: return nil
        }
    }

    static func list(candidates: [Candidate], inventory: StorageInventoryReportDTO?,
                     filter: DiscoveryFilter, query: String, sort: DiscoverySort) -> [Self] {
        let cleanup = candidates.filter { filter.matches($0) && $0.matchesSearch(query) }.map(Self.candidate)
        let managed = (inventory?.rows ?? []).filter { row in
            row.matches(filter: filter, query: query) && !candidates.contains { candidate in
                // Suppress aggregate observations when the scanner has an exact
                // finding within them, even when that finding is filtered out.
                candidate.path == row.path || candidate.path.hasPrefix(row.path + "/")
                    || row.path.hasPrefix(candidate.path + "/")
            }
        }.map(Self.managed)
        let rows = cleanup + managed
        switch sort {
        case .suggested:
            // Preserve the engine's ranking, then show tool-owned observations.
            return rows
        case .largest:
            return rows.sorted { lhs, rhs in
                switch (lhs.allocatedBytes, rhs.allocatedBytes) {
                case (let l?, let r?) where l != r: return l > r
                case (_?, nil): return true
                case (nil, _?): return false
                default: return lhs.path == rhs.path ? lhs.id < rhs.id : lhs.path < rhs.path
                }
            }
        case .oldest:
            return rows.sorted { lhs, rhs in
                switch (lhs.modifiedNs, rhs.modifiedNs) {
                case (let l?, let r?) where l != r: return l < r
                case (_?, nil): return true
                case (nil, _?): return false
                default: return lhs.path == rhs.path ? lhs.id < rhs.id : lhs.path < rhs.path
                }
            }
        case .name:
            return rows.sorted { lhs, rhs in
                let order = lhs.title.localizedStandardCompare(rhs.title)
                return order == .orderedSame ? lhs.id < rhs.id : order == .orderedAscending
            }
        }
    }
}

struct StorageInventoryStatus: View {
    let report: StorageInventoryReportDTO?
    let isLoading: Bool
    let errorMessage: String?
    let refresh: () -> Void
    @State private var showIssues = false

    var body: some View {
        VStack(alignment: .leading, spacing: 7) {
            if isLoading {
                HStack(spacing: 7) {
                    ProgressView().controlSize(.small)
                    Text("Checking other storage locations…")
                        .font(TeaFont.caption)
                        .foregroundStyle(TeaTheme.inkSoft)
                }
                .accessibilityElement(children: .combine)
            }
            if let errorMessage {
                Text(errorMessage)
                    .font(TeaFont.caption)
                    .foregroundStyle(TeaTheme.rust)
                    .fixedSize(horizontal: false, vertical: true)
                    .textSelection(.enabled)
                Button("Try again", action: refresh)
                    .buttonStyle(.plain)
                    .font(TeaFont.captionMedium)
                    .foregroundStyle(TeaTheme.biro)
                    .disabled(isLoading)
            }
            if let report, !report.issues.isEmpty || report.omittedIssues > 0 {
                DisclosureGroup(isExpanded: $showIssues) {
                    VStack(alignment: .leading, spacing: 7) {
                        ForEach(Array(report.issues.enumerated()), id: \.offset) { _, issue in
                            VStack(alignment: .leading, spacing: 3) {
                                Text(issue.detail)
                                    .font(TeaFont.caption)
                                    .fixedSize(horizontal: false, vertical: true)
                                Text(issue.path)
                                    .font(TeaFont.mono)
                                    .lineLimit(2)
                                    .truncationMode(.middle)
                                    .textSelection(.enabled)
                                    .help(issue.path)
                            }
                        }
                        if report.omittedIssues > 0 {
                            Text("\(report.omittedIssues) additional entries could not be measured.")
                                .font(TeaFont.caption)
                                .fixedSize(horizontal: false, vertical: true)
                        }
                        Button("Check again", action: refresh)
                            .buttonStyle(.plain)
                            .font(TeaFont.captionMedium)
                            .foregroundStyle(TeaTheme.biro)
                            .disabled(isLoading)
                    }
                    .foregroundStyle(TeaTheme.inkSoft)
                    .padding(.top, 5)
                } label: {
                    Label("Some locations were not fully measured", systemImage: "info.circle")
                        .font(TeaFont.captionMedium)
                        .foregroundStyle(TeaTheme.rust)
                }
            }
        }
    }
}

struct StorageInventoryRowView: View {
    let row: StorageInventoryReportDTO.Row
    let openProvider: (ManagedProviderID) -> Void
    @State private var expanded = false

    var body: some View {
        InkCard(padding: 9, seed: 431, fill: TeaTheme.paperDeep.opacity(0.55), stroke: TeaTheme.ink.opacity(0.25)) {
            DisclosureGroup(isExpanded: $expanded) {
                VStack(alignment: .leading, spacing: 6) {
                    Text(row.path)
                        .font(TeaFont.mono)
                        .lineLimit(3)
                        .truncationMode(.middle)
                        .textSelection(.enabled)
                        .help(row.path)
                    if row.state == .partial {
                        Text("Partial measurement. The total for this location is unknown.")
                            .foregroundStyle(TeaTheme.rust)
                    }
                    Text(row.detail)
                    Text(row.ownerFollowup)
                    if let owner = row.owner {
                        Button("Review with " + owner.title) { openProvider(owner) }
                            .buttonStyle(.plain)
                            .font(TeaFont.captionMedium)
                            .foregroundStyle(TeaTheme.biro)
                            .accessibilityHint("Opens the read-only owner review. It does not remove files.")
                    }
                }
                .font(TeaFont.caption)
                .foregroundStyle(TeaTheme.inkSoft)
                .fixedSize(horizontal: false, vertical: true)
                .padding(.top, 6)
            } label: {
                VStack(alignment: .leading, spacing: 3) {
                    HStack(alignment: .firstTextBaseline, spacing: 6) {
                        Text(row.title)
                            .font(TeaFont.captionSemibold)
                            .fixedSize(horizontal: false, vertical: true)
                        Spacer(minLength: 3)
                        Text(row.sizeLabel)
                            .font(TeaFont.captionMedium)
                            .monospacedDigit()
                            .foregroundStyle(row.state == .complete ? TeaTheme.ink : TeaTheme.rust)
                    }
                    Text(row.category)
                        .font(TeaFont.caption)
                        .foregroundStyle(TeaTheme.inkSoft)
                }
                .accessibilityElement(children: .combine)
            }
        }
    }
}
