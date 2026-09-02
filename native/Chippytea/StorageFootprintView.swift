import SwiftUI

/// The storage endpoint reports the derived index separately from the SQLite
/// file, which also contains durable scan, cleanup, root and wallet truth.
/// Keeping these fields distinct prevents the UI from presenting a database
/// size as an amount that can be reclaimed.
struct StorageFootprintUsageDTO: Decodable, Equatable {
    let candidateRows: UInt64
    let candidatePayloadBytes: UInt64
    let databaseBytes: UInt64
    let walBytes: UInt64
    let walShmBytes: UInt64
    let walFrames: UInt64
    let walCheckpointed: UInt64
    let walBusy: Bool
}

/// A small, explicit control surface for Chippytea's own local index. It does
/// not scan the filesystem, inspect user caches, or claim that database bytes
/// are recoverable space.
struct StorageFootprintView: View {
    @ObservedObject var model: AppModel
    @State private var usage: StorageFootprintUsageDTO?
    @State private var isRefreshing = false
    @State private var isMaintaining = false
    @State private var didAppear = false
    @State private var errorMessage: String?

    private var isBusy: Bool { isRefreshing || isMaintaining }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("Local index footprint")
                .font(TeaFont.bodySemibold)
            Text("New derived findings use budgets of 50,000 rows and 32 MiB of payload. Existing entries are trimmed gradually. Protected records, cleanup history, recovery receipts, roots, Keep decisions, and chips stay preserved.")
                .font(TeaFont.caption)
                .foregroundStyle(TeaTheme.inkSoft)
                .fixedSize(horizontal: false, vertical: true)
            Text("This reads only Chippytea’s local database; it does not scan files or inspect user caches.")
                .font(TeaFont.caption)
                .foregroundStyle(TeaTheme.inkSoft)
                .fixedSize(horizontal: false, vertical: true)

            HStack(spacing: 8) {
                Button {
                    refreshUsage()
                } label: {
                    Label(isRefreshing ? "Refreshing…" : "Refresh footprint", systemImage: "arrow.clockwise")
                }
                .buttonStyle(InkButtonStyle(kind: .quiet, compact: true, seed: 411))
                .disabled(isBusy)

                Button {
                    maintainStorage()
                } label: {
                    Label(isMaintaining ? "Tidying…" : "Tidy local index", systemImage: "sparkles")
                }
                .buttonStyle(InkButtonStyle(kind: .quiet, compact: true, seed: 413))
                .disabled(isBusy)
            }
            .accessibilityElement(children: .contain)

            if isBusy {
                HStack(spacing: 7) {
                    ProgressView().controlSize(.small)
                    Text(isMaintaining ? "Tidying the local index…" : "Reading the local index…")
                        .font(TeaFont.caption)
                        .foregroundStyle(TeaTheme.inkSoft)
                }
            }

            if let errorMessage {
                Text(errorMessage)
                    .font(TeaFont.caption)
                    .foregroundStyle(TeaTheme.rust)
                    .fixedSize(horizontal: false, vertical: true)
                    .textSelection(.enabled)
            }

            if let usage {
                InkDivider(seed: 415)
                footprintMetric("Candidate rows", usage.candidateRows.formatted())
                footprintMetric("Candidate payload", space(usage.candidatePayloadBytes))
                footprintMetric("SQLite database (durable + derived)", space(usage.databaseBytes))
                footprintMetric("WAL", space(usage.walBytes))
                footprintMetric("WAL shared memory", space(usage.walShmBytes))
                if usage.walBusy {
                    Text("The WAL could not be fully tidied while in use; it remains accounted for and was not forced.")
                        .font(TeaFont.caption)
                        .foregroundStyle(TeaTheme.inkSoft)
                        .fixedSize(horizontal: false, vertical: true)
                }
                Text("These are local storage measurements, not recovered space or chips. The database line includes durable truth and is not a quota.")
                    .font(TeaFont.caption)
                    .foregroundStyle(TeaTheme.inkSoft)
                    .fixedSize(horizontal: false, vertical: true)
            } else if !isBusy && errorMessage == nil {
                Text("No footprint measurement is loaded yet.")
                    .font(TeaFont.caption)
                    .foregroundStyle(TeaTheme.inkSoft)
            }
        }
        .onAppear {
            guard !didAppear else { return }
            didAppear = true
            refreshUsage()
        }
    }

    private func footprintMetric(_ title: String, _ value: String) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Text(title)
                .font(TeaFont.caption)
                .foregroundStyle(TeaTheme.inkSoft)
            Spacer(minLength: 4)
            Text(value)
                .font(TeaFont.mono)
                .monospacedDigit()
        }
    }

    private func refreshUsage() {
        guard !isBusy else { return }
        guard let client = model.client else {
            usage = nil
            errorMessage = "The local engine is not ready, so no footprint measurement is available."
            return
        }
        isRefreshing = true
        usage = nil
        errorMessage = nil
        Task { @MainActor in
            defer { isRefreshing = false }
            do {
                let data = try await client.request(["action": "storage_usage"])
                usage = try EngineClient.decode(StorageFootprintUsageDTO.self, data)
            } catch {
                usage = nil
                errorMessage = error.localizedDescription
            }
        }
    }

    private func maintainStorage() {
        guard !isBusy else { return }
        guard let client = model.client else {
            usage = nil
            errorMessage = "The local engine is not ready, so local-index maintenance cannot start."
            return
        }
        isMaintaining = true
        usage = nil
        errorMessage = nil
        Task { @MainActor in
            defer { isMaintaining = false }
            do {
                let data = try await client.request(["action": "maintain_storage"])
                usage = try EngineClient.decode(StorageFootprintUsageDTO.self, data)
            } catch {
                usage = nil
                errorMessage = error.localizedDescription
            }
            // Eviction can commit before a later checkpoint or response fails.
            // Refresh findings and coverage even on that partial-success path;
            // idle discovery polling is not responsible for this mutation.
            await model.reload()
        }
    }
}
