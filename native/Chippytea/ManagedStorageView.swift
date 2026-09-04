import Foundation
import SwiftUI

/// The owner-review response is intentionally separate from the cleanup models.
/// It can describe what a tool knows without becoming permission to run that tool's
/// cleanup command.
struct ManagedProviderReviewDTO: Decodable {
    enum Provider: String, Decodable, Equatable {
        case homebrew = "Homebrew"
        case uv = "Uv"
        case pnpm = "Pnpm"
        case dockerBuildKit = "DockerBuildKit"
        case vsCodeExtensions = "VsCodeExtensions"
        case cursorExtensions = "CursorExtensions"
    }

    enum State: String, Decodable, Equatable {
        case complete = "Complete"
        case partial = "Partial"
        case unknown = "Unknown"

        var title: String {
            switch self {
            case .complete: return "Complete owner evidence"
            case .partial: return "Partial owner evidence"
            case .unknown: return "Owner evidence unavailable"
            }
        }

        var color: Color {
            switch self {
            case .complete: return TeaTheme.goldDeep
            case .partial: return TeaTheme.rust
            case .unknown: return TeaTheme.inkSoft
            }
        }
    }

    enum CleanupAuthority: String, Decodable, Equatable {
        case reviewOnly = "ReviewOnly"
    }

    struct Evidence: Decodable {
        let source: String
        let detail: String
        let verified: Bool
    }

    struct Observation: Decodable {
        let id: String
        let description: String
        let logicalBytes: UInt64?
        let hostBytes: UInt64?
        let reclaimable: Bool?
        let evidence: [Evidence]
    }

    let provider: Provider
    let state: State
    let observations: [Observation]
    let logicalRecoveryBytes: UInt64
    let hostRecoveryBytes: UInt64?
    let evidence: [Evidence]
    let consequence: String
    let ownerFollowup: String
    let cleanupAuthority: CleanupAuthority

    /// Decode through the same snake-case decoder as EngineClient, then require
    /// the response to match the user-selected fixed provider and review-only
    /// authority. Unknown enum values and unsafe authority values fail closed.
    static func decode(_ data: Data, expectedProvider: ManagedProviderID) throws -> Self {
        let decoded = try EngineClient.decode(Self.self, data)
        guard decoded.provider == expectedProvider.responseProvider,
              decoded.cleanupAuthority == .reviewOnly else {
            throw ManagedStorageReviewError.invalidResponse
        }
        return decoded
    }
}

enum ManagedProviderID: String, CaseIterable, Identifiable, Hashable {
    case homebrew
    case uv
    case pnpm
    case docker
    case vscodeExtensions = "vscode_extensions"
    case cursorExtensions = "cursor_extensions"

    var id: String { rawValue }

    var title: String {
        switch self {
        case .homebrew: return "Homebrew"
        case .uv: return "uv"
        case .pnpm: return "pnpm"
        case .docker: return "Docker BuildKit"
        case .vscodeExtensions: return "VS Code obsolete extensions"
        case .cursorExtensions: return "Cursor obsolete extensions"
        }
    }

    var responseProvider: ManagedProviderReviewDTO.Provider {
        switch self {
        case .homebrew: return .homebrew
        case .uv: return .uv
        case .pnpm: return .pnpm
        case .docker: return .dockerBuildKit
        case .vscodeExtensions: return .vsCodeExtensions
        case .cursorExtensions: return .cursorExtensions
        }
    }
}

/// A deliberately explicit, read-only look at caches owned by installed tools.
/// Nothing runs when this view appears, and the result contains no cleanup action.
struct ManagedStorageView: View {
    @ObservedObject var model: AppModel
    @State private var selectedProvider: ManagedProviderID = .homebrew
    @State private var loadingProvider: ManagedProviderID?
    @State private var cancellingProvider: ManagedProviderID?
    @State private var review: ManagedProviderReviewDTO?
    @State private var errorMessage: String?
    @State private var publicationToken = UUID()

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("Some tools keep their own caches outside chippytea’s index.")
                .font(TeaFont.bodyMedium)
            Text("Choose an installed owner to request a bounded, read-only review. Nothing is deleted, pruned, or counted as chips.")
                .font(TeaFont.caption)
                .foregroundStyle(TeaTheme.inkSoft)
                .fixedSize(horizontal: false, vertical: true)
            Text("Uses global settings visible to chippytea, not project settings or shell profiles. Installed tools cannot write or use the network. Docker uses a fixed read-only request to its default local context.")
                .font(TeaFont.caption)
                .foregroundStyle(TeaTheme.inkSoft)
                .fixedSize(horizontal: false, vertical: true)

            Picker("Owner", selection: $selectedProvider) {
                ForEach(ManagedProviderID.allCases) { provider in
                    Text(provider.title).tag(provider)
                }
            }
            .pickerStyle(.menu)
            .accessibilityLabel("Owner-managed tool")
            .disabled(loadingProvider != nil || cancellingProvider != nil)

            Button {
                requestReview(for: selectedProvider)
            } label: {
                if let cancellingProvider {
                    Label("Stopping " + cancellingProvider.title + "…", systemImage: "hourglass")
                } else if let loadingProvider {
                    Label("Reviewing " + loadingProvider.title + "…", systemImage: "hourglass")
                } else {
                    Label("Review " + selectedProvider.title + " now", systemImage: "magnifyingglass")
                }
            }
            .buttonStyle(InkButtonStyle(kind: .quiet, fullWidth: true, compact: true, seed: 397))
            .disabled(model.client == nil || loadingProvider != nil || cancellingProvider != nil)
            .accessibilityHint("Asks the selected tool for read-only cache information. It does not delete files or start a cleanup.")

            if let cancellingProvider {
                Text("Stopping " + cancellingProvider.title + " review…")
                    .font(TeaFont.captionMedium)
                    .foregroundStyle(TeaTheme.inkSoft)
            } else if let loadingProvider {
                Button("Stop " + loadingProvider.title + " review") { cancelReview() }
                .buttonStyle(.plain)
                .font(TeaFont.captionMedium)
                .foregroundStyle(TeaTheme.biro)
                .accessibilityHint("Stops this owner review only. Any ordinary scan continues.")
            }

            if model.client == nil {
                Text("The local engine is starting. You can review an owner once it is ready.")
                    .font(TeaFont.caption)
                    .foregroundStyle(TeaTheme.inkSoft)
                    .fixedSize(horizontal: false, vertical: true)
            }

            if let errorMessage {
                managedReviewError(errorMessage)
            }

            if let review {
                ManagedProviderResult(review: review)
            }
        }
        .onAppear { selectedProvider = model.managedProvider }
        .onChange(of: model.managedProvider) { _, provider in
            guard loadingProvider == nil, cancellingProvider == nil else { return }
            selectedProvider = provider
        }
        .onChange(of: selectedProvider) { _, _ in
            guard loadingProvider == nil, cancellingProvider == nil else { return }
            review = nil
            errorMessage = nil
        }
        .onDisappear {
            // Cancel only this view's owner review. This never cancels an ordinary scan.
            if loadingProvider != nil, cancellingProvider == nil {
                cancelReview()
            } else {
                publicationToken = UUID()
            }
        }
    }

    private func requestReview(for provider: ManagedProviderID) {
        guard loadingProvider == nil, cancellingProvider == nil, let client = model.client else { return }
        let token = UUID()
        publicationToken = token
        loadingProvider = provider
        review = nil
        errorMessage = nil

        Task { @MainActor in
            do {
                let data = try await client.reviewManagedProvider(provider.rawValue, requestID: token.uuidString)
                let decoded = try ManagedProviderReviewDTO.decode(data, expectedProvider: provider)
                guard publicationToken == token else { return }
                review = decoded
                loadingProvider = nil
            } catch {
                guard publicationToken == token else { return }
                errorMessage = error.localizedDescription
                loadingProvider = nil
            }
        }
    }

    private func cancelReview() {
        guard let provider = loadingProvider, cancellingProvider == nil else {
            publicationToken = UUID()
            return
        }
        let requestID = publicationToken.uuidString
        publicationToken = UUID()
        cancellingProvider = provider
        guard let client = model.client else {
            loadingProvider = nil
            cancellingProvider = nil
            return
        }
        Task { @MainActor in
            await client.cancelManagedReview(requestID)
            guard cancellingProvider == provider else { return }
            loadingProvider = nil
            cancellingProvider = nil
        }
    }

    private func managedReviewError(_ message: String) -> some View {
        InkCard(padding: 9, seed: 399, fill: TeaTheme.card, stroke: TeaTheme.rust) {
            HStack(alignment: .top, spacing: 8) {
                Image(systemName: "exclamationmark.circle")
                    .foregroundStyle(TeaTheme.rust)
                Text(message)
                    .font(TeaFont.caption)
                    .fixedSize(horizontal: false, vertical: true)
                    .textSelection(.enabled)
                Spacer(minLength: 0)
            }
        }
    }
}

private enum ManagedStorageReviewError: LocalizedError {
    case invalidResponse

    var errorDescription: String? {
        "The owner returned an unexpected or non-review response. No cleanup authority was granted."
    }
}

private struct ManagedProviderResult: View {
    let review: ManagedProviderReviewDTO

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            InkDivider(seed: 401)
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text("Latest " + providerTitle).font(TeaFont.bodySemibold)
                Spacer(minLength: 4)
                Text(review.state.title)
                    .font(TeaFont.captionSemibold)
                    .foregroundStyle(review.state.color)
                    .padding(.horizontal, 6)
                    .padding(.vertical, 3)
                    .background(WobblyPill(seed: 403).fill(review.state.color.opacity(0.12)))
                    .overlay(WobblyPill(seed: 403).stroke(review.state.color.opacity(0.55), lineWidth: 1))
            }

            Text("Logical space named by the owner: " + (review.logicalRecoveryBytes > 0 ? space(review.logicalRecoveryBytes) : "not supplied"))
                .font(TeaFont.caption)
                .foregroundStyle(TeaTheme.inkSoft)
            Text("Estimated host recovery: " + (review.hostRecoveryBytes.map(space) ?? "not established"))
                .font(TeaFont.caption)
                .foregroundStyle(TeaTheme.inkSoft)
            Text("These are observations only. They do not represent physical recovery or chips.")
                .font(TeaFont.caption)
                .foregroundStyle(TeaTheme.inkSoft)
                .fixedSize(horizontal: false, vertical: true)

            if !review.observations.isEmpty {
                Text("Owner observations")
                    .font(TeaFont.captionSemibold)
                    .padding(.top, 2)
                ForEach(Array(review.observations.prefix(256).enumerated()), id: \.offset) { index, observation in
                    ManagedObservationRow(observation: observation, seed: 405 + index * 2)
                }
            }

            InkDivider(seed: 407)
            Text("What this means").font(TeaFont.captionSemibold)
            Text(review.consequence)
                .font(TeaFont.caption)
                .foregroundStyle(TeaTheme.inkSoft)
                .fixedSize(horizontal: false, vertical: true)
            Text("Owner follow-up").font(TeaFont.captionSemibold)
            Text(review.ownerFollowup)
                .font(TeaFont.caption)
                .foregroundStyle(TeaTheme.inkSoft)
                .fixedSize(horizontal: false, vertical: true)
            ManagedEvidenceList(evidence: review.evidence)
        }
    }

    private var providerTitle: String {
        switch review.provider {
        case .homebrew: return "Homebrew"
        case .uv: return "uv"
        case .pnpm: return "pnpm"
        case .dockerBuildKit: return "Docker BuildKit"
        case .vsCodeExtensions: return "VS Code obsolete extensions"
        case .cursorExtensions: return "Cursor obsolete extensions"
        }
    }
}

private struct ManagedObservationRow: View {
    let observation: ManagedProviderReviewDTO.Observation
    let seed: Int

    var body: some View {
        InkCard(padding: 8, seed: seed, fill: TeaTheme.paperDeep.opacity(0.55), stroke: TeaTheme.ink.opacity(0.25)) {
            VStack(alignment: .leading, spacing: 4) {
                Text(observation.description)
                    .font(TeaFont.captionMedium)
                    .fixedSize(horizontal: false, vertical: true)
                HStack(spacing: 8) {
                    if let bytes = observation.logicalBytes {
                        Text("Logical " + space(bytes)).font(TeaFont.mono)
                    }
                    Text(reclaimability)
                        .font(TeaFont.caption)
                        .foregroundStyle(observation.reclaimable == true ? TeaTheme.goldDeep : TeaTheme.inkSoft)
                }
                if let hostBytes = observation.hostBytes {
                    Text("Host " + space(hostBytes))
                        .font(TeaFont.mono)
                        .foregroundStyle(TeaTheme.inkSoft)
                }
                ManagedEvidenceList(evidence: observation.evidence)
            }
        }
    }

    private var reclaimability: String {
        switch observation.reclaimable {
        case .some(true): return "Owner marked for review"
        case .some(false): return "Owner did not mark reclaimable"
        case .none: return "Reclaimability unknown"
        }
    }
}

private struct ManagedEvidenceList: View {
    let evidence: [ManagedProviderReviewDTO.Evidence]

    var body: some View {
        if !evidence.isEmpty {
            VStack(alignment: .leading, spacing: 3) {
                ForEach(Array(evidence.prefix(8).enumerated()), id: \.offset) { _, item in
                    HStack(alignment: .top, spacing: 5) {
                        Image(systemName: item.verified ? "checkmark.seal" : "questionmark.circle")
                            .font(TeaFont.caption)
                            .foregroundStyle(item.verified ? TeaTheme.goldDeep : TeaTheme.inkSoft)
                        if item.source == "official-documentation",
                           let url = URL(string: item.detail),
                           url.scheme == "https" {
                            Link("Official documentation", destination: url)
                                .font(TeaFont.caption)
                        } else {
                            Text(item.detail)
                                .font(TeaFont.mono)
                                .foregroundStyle(TeaTheme.inkSoft)
                                .lineLimit(2)
                                .truncationMode(.middle)
                        }
                    }
                }
            }
            .padding(.top, 2)
        }
    }
}
