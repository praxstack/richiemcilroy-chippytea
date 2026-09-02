import SwiftUI

struct DuplicateCheckProgress: Decodable, Equatable {
    let phase: String
    let filesConsidered: Int
    let filesCompared: Int
    let skippedFiles: Int
    let bytesRead: UInt64
    let groupsFound: Int
    let limited: Bool
    let cancelled: Bool
    let complete: Bool
}

struct DuplicateFile: Decodable, Identifiable, Equatable {
    let candidate: Candidate
    let keeperOnly: Bool
    var id: String { candidate.id }
}

struct DuplicateGroup: Decodable, Identifiable, Equatable {
    let id: String
    let files: [DuplicateFile]
}

struct DuplicateReport: Decodable, Equatable {
    let token: String
    let groups: [DuplicateGroup]
    let progress: DuplicateCheckProgress
    let indexedFiles: UInt64
    let skippedBuckets: UInt64
    let skippedBucketFiles: UInt64
    let bucketLimitReached: Bool
    let expiresInSeconds: UInt64
}

/// An opt-in content check, separate from ordinary metadata-only discovery.
/// Choices belong to one report; neither a keeper nor a copy is ever preselected.
struct DuplicateReviewPage: View {
    @ObservedObject var model: AppModel
    @State private var selectionToken: String?
    @State private var keeperIDs: [String: String] = [:]
    @State private var copyIDs: [String: String] = [:]
    @State private var activeGroupID: String?

    private var workInProgress: Bool {
        model.busy || model.hasCleanupWork || model.snapshot.scanning
            || model.snapshot.cleaning || model.duplicateChecking
    }

    private var currentError: String? {
        guard let message = model.errorMessage, !message.isEmpty else { return nil }
        return message
    }

    var body: some View {
        VStack(spacing: 0) {
            header
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 14) {
                    introduction
                    if let error = currentError { errorNotice(error) }
                    if model.duplicateChecking {
                        progressNotice
                    } else {
                        safetyNotice
                        if let report = model.duplicateReport {
                            reportSummary(report)
                            if !report.groups.isEmpty {
                                Text("Choose both a keeper and one copy in a group. Keeping a file here applies only to this cleanup, not your saved Keep preferences.")
                                    .foregroundStyle(TeaTheme.inkSoft)
                                    .fixedSize(horizontal: false, vertical: true)
                                ForEach(report.groups) { group in
                                    groupSection(group, report: report)
                                }
                            }
                        }
                    }
                }
                .padding(.horizontal, TeaTheme.panelPadding)
                .padding(.top, 10).padding(.bottom, 14)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            .scrollIndicators(.hidden)
            footer
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .font(TeaFont.body)
        .foregroundStyle(TeaTheme.ink)
        .background(TeaTheme.paper)
        .onAppear { resetChoices(for: model.duplicateReport?.token) }
        .onChange(of: model.duplicateReport?.token) { _, token in
            resetChoices(for: token)
        }
    }

    private var header: some View {
        HStack(spacing: 8) {
            Button { model.closeDuplicates() } label: {
                Label("Back", systemImage: "chevron.left")
            }
            .buttonStyle(InkButtonStyle(kind: .quiet, compact: true, seed: 901))
            .accessibilityLabel("Close matching-file review")
            Spacer(minLength: 4)
            Text("Content check")
                .font(TeaFont.caption).foregroundStyle(TeaTheme.inkSoft)
        }
        .padding(.horizontal, TeaTheme.panelPadding)
        .padding(.top, 10).padding(.bottom, 2)
    }

    private var introduction: some View {
        VStack(alignment: .leading, spacing: 8) {
            ScreenTitle(text: "Check matching files.", seed: 903, font: TeaFont.headline)
                .fixedSize(horizontal: false, vertical: true)
                .accessibilityAddTraits(.isHeader)
            if model.duplicateReport == nil && !model.duplicateChecking {
                Text("Check files explicitly reads the contents of your current indexed personal-file suggestions: old or large downloads and large personal files. It is not a whole-Mac search. Normal scanning stays metadata-only for these files.")
                    .foregroundStyle(TeaTheme.inkSoft)
                    .fixedSize(horizontal: false, vertical: true)
                Text("Each check is bounded and selects complete same-size groups. Some groups may be skipped.")
                    .font(TeaFont.caption).foregroundStyle(TeaTheme.inkSoft)
                    .fixedSize(horizontal: false, vertical: true)
            } else {
                Text("A content check of current indexed personal-file suggestions, not your whole Mac.")
                    .foregroundStyle(TeaTheme.inkSoft)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    private var safetyNotice: some View {
        InkCard(seed: 905) {
            VStack(alignment: .leading, spacing: 6) {
                Text("Trash only · 0 chips").font(TeaFont.bodySemibold)
                Text("Matching content does not tell you whether both paths are needed. APFS can share blocks, so file sizes are not a promise of freed space.")
                    .font(TeaFont.caption).foregroundStyle(TeaTheme.inkSoft)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    private func errorNotice(_ message: String) -> some View {
        InkCard(seed: 907, stroke: TeaTheme.rust) {
            VStack(alignment: .leading, spacing: 6) {
                Text("Check needs attention").font(TeaFont.bodySemibold)
                Text(message).textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
                Text("Resolve the issue and check again before reviewing a copy.")
                    .font(TeaFont.caption).foregroundStyle(TeaTheme.inkSoft)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    private var progressNotice: some View {
        InkCard(seed: 909) {
            VStack(alignment: .leading, spacing: 8) {
                if let progress = model.duplicateProgress {
                    Text(phaseLabel(progress.phase)).font(TeaFont.subtitle)
                        .fixedSize(horizontal: false, vertical: true)
                    Text("\(progress.filesCompared) files compared · \(progress.filesConsidered) considered")
                        .monospacedDigit().fixedSize(horizontal: false, vertical: true)
                    Text("\(space(progress.bytesRead)) read · \(progress.groupsFound) matching groups")
                        .monospacedDigit().fixedSize(horizontal: false, vertical: true)
                    if progress.skippedFiles > 0 {
                        Text("\(progress.skippedFiles) files skipped")
                            .font(TeaFont.caption).foregroundStyle(TeaTheme.inkSoft)
                    }
                } else {
                    Text("Starting content check…").font(TeaFont.subtitle)
                }
                Text("Checking changes no files. You can cancel at any time.")
                    .font(TeaFont.caption).foregroundStyle(TeaTheme.inkSoft)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .accessibilityElement(children: .combine)
    }

    private func reportSummary(_ report: DuplicateReport) -> some View {
        VStack(alignment: .leading, spacing: 7) {
            Text(reportTitle(report)).font(TeaFont.subtitle)
                .accessibilityAddTraits(.isHeader)
            Text("\(report.indexedFiles) indexed suggestions · \(report.progress.filesCompared) files compared · \(space(report.progress.bytesRead)) read")
                .font(TeaFont.caption).monospacedDigit().foregroundStyle(TeaTheme.inkSoft)
                .fixedSize(horizontal: false, vertical: true)
            if report.groups.isEmpty {
                Text(isPartial(report)
                     ? "No matching groups were returned from this partial check. Other indexed files may still match."
                     : "No matching content was found among the files compared. This does not rule out matches elsewhere.")
                    .fixedSize(horizontal: false, vertical: true)
            }
            if report.progress.cancelled {
                Text("You cancelled this check. Only matches completed before cancellation are shown.")
                    .fixedSize(horizontal: false, vertical: true)
            } else if !report.progress.complete {
                Text("This check did not finish. The report does not cover every eligible size group.")
                    .fixedSize(horizontal: false, vertical: true)
            }
            if report.progress.limited {
                Text("The check reached a work limit. This is a partial report, not a complete search.")
                    .fixedSize(horizontal: false, vertical: true)
            }
            if report.skippedBuckets > 0 || report.skippedBucketFiles > 0 {
                Text("\(report.skippedBuckets) same-size groups (\(report.skippedBucketFiles) files) were skipped to keep the check bounded.")
                    .fixedSize(horizontal: false, vertical: true)
            }
            if report.bucketLimitReached {
                Text("The size-group limit was reached; other size groups may not be represented.")
                    .fixedSize(horizontal: false, vertical: true)
            }
            if report.progress.skippedFiles > 0 {
                Text("\(report.progress.skippedFiles) files were skipped during the check.")
                    .fixedSize(horizontal: false, vertical: true)
            }
            Text(report.expiresInSeconds == 0
                 ? "This report has expired. Check again before reviewing a copy."
                 : "Results are temporary. Changed files or expired evidence require a fresh check before cleanup.")
                .font(TeaFont.caption).foregroundStyle(TeaTheme.inkSoft)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    private func groupSection(_ group: DuplicateGroup, report: DuplicateReport) -> some View {
        LazyVStack(alignment: .leading, spacing: 10) {
            Text("Matching files (\(group.files.count))")
                .font(TeaFont.subtitle).accessibilityAddTraits(.isHeader)
            ForEach(group.files) { file in
                InkCard(seed: 911) {
                    fileRow(file, group: group, report: report)
                }
            }
            Button("Review selected copy") {
                reviewSelection(in: group, report: report)
            }
            .buttonStyle(InkButtonStyle(kind: .primary, fullWidth: true, compact: true, seed: 917))
            .disabled(!canReview(group, report: report))
            .accessibilityHint("Opens review for one selected copy to move to Trash. Your chosen keeper stays in place.")
            if keeperIDs[group.id] == nil || copyIDs[group.id] == nil {
                Text("Choose one keeper and one different copy to continue.")
                    .font(TeaFont.caption).foregroundStyle(TeaTheme.inkSoft)
                    .fixedSize(horizontal: false, vertical: true)
            }
            InkDivider(seed: 915)
        }
    }

    private func fileRow(_ file: DuplicateFile, group: DuplicateGroup, report: DuplicateReport) -> some View {
        let keeperChosen = keeperIDs[group.id] == file.id
        let copyChosen = copyIDs[group.id] == file.id
        let copyAllowed = !file.keeperOnly && file.candidate.canReviewCleanup
        return VStack(alignment: .leading, spacing: 7) {
            HStack(alignment: .top, spacing: 8) {
                Image(systemName: file.candidate.symbol)
                    .foregroundStyle(TeaTheme.inkSoft).accessibilityHidden(true)
                Text(file.candidate.title).font(TeaFont.bodySemibold)
                    .fixedSize(horizontal: false, vertical: true)
                    .frame(maxWidth: .infinity, alignment: .leading)
                Button { model.reveal(path: file.candidate.path) } label: {
                    Image(systemName: "folder")
                }
                .buttonStyle(InkIconButtonStyle(seed: 919))
                .help("Reveal in Finder")
                .accessibilityLabel("Reveal \(file.candidate.path) in Finder")
            }
            Text(file.candidate.path).font(TeaFont.mono).textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)
                .frame(maxWidth: .infinity, alignment: .leading)
            Text("\(space(file.candidate.logicalBytes)) file size · \(file.candidate.category)")
                .font(TeaFont.caption).foregroundStyle(TeaTheme.inkSoft)
                .fixedSize(horizontal: false, vertical: true)
            HStack(spacing: 8) {
                Button { chooseKeeper(file.id, group: group, report: report) } label: {
                    choiceLabel("Keep for this cleanup", chosen: keeperChosen)
                }
                .buttonStyle(InkButtonStyle(kind: .quiet, fullWidth: true, compact: true, seed: 921))
                .disabled(!choicesEnabled(for: report))
                .accessibilityLabel("Keep \(file.candidate.path) for this cleanup")
                .accessibilityValue(keeperChosen ? "Chosen" : "Not chosen")
                .accessibilityHint("Does not change your saved Keep preferences.")
                Button { chooseCopy(file.id, group: group, report: report) } label: {
                    choiceLabel(copyChosen ? "Copy selected" : "Select copy", chosen: copyChosen)
                }
                .buttonStyle(InkButtonStyle(kind: .quiet, fullWidth: true, compact: true, seed: 923))
                .disabled(!choicesEnabled(for: report) || !copyAllowed || keeperChosen)
                .accessibilityLabel("Select \(file.candidate.path) as the copy to review")
                .accessibilityValue(copyChosen ? "Chosen" : "Not chosen")
            }
            if !copyAllowed {
                Text(file.keeperOnly
                     ? "Keep-only file: it cannot be selected as a copy."
                     : file.candidate.cleanupBlockedReason ?? "This file cannot be selected as a copy.")
                    .font(TeaFont.caption).foregroundStyle(TeaTheme.inkSoft)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    private func choiceLabel(_ title: String, chosen: Bool) -> some View {
        HStack(spacing: 4) {
            Image(systemName: chosen ? "checkmark.circle.fill" : "circle")
            Text(title).fixedSize(horizontal: false, vertical: true)
        }
        .foregroundStyle(chosen ? TeaTheme.biro : TeaTheme.ink)
    }

    private var footer: some View {
        VStack(spacing: 7) {
            if model.duplicateChecking {
                Button("Cancel check") { model.cancel() }
                    .buttonStyle(InkButtonStyle(kind: .quiet, fullWidth: true, compact: true, seed: 925))
                    .accessibilityLabel("Cancel the matching-file content check")
            } else {
                if let report = model.duplicateReport, !report.groups.isEmpty {
                    let group = report.groups.first { $0.id == activeGroupID }
                    if let group, let copyID = copyIDs[group.id],
                       let copy = group.files.first(where: { $0.id == copyID }) {
                        Text("Copy to review: \(copy.candidate.title)")
                            .font(TeaFont.caption).foregroundStyle(TeaTheme.inkSoft)
                            .lineLimit(1).truncationMode(.middle)
                            .help(copy.candidate.path)
                            .accessibilityLabel("Copy to review: \(copy.candidate.path)")
                    } else {
                        Text("Choose one keeper and one different copy in the same group.")
                            .font(TeaFont.caption).foregroundStyle(TeaTheme.inkSoft)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                    Button("Review selected copy") {
                        if let group { reviewSelection(in: group, report: report) }
                    }
                    .buttonStyle(InkButtonStyle(kind: .primary, fullWidth: true, compact: true, seed: 917))
                    .disabled(group.map { !canReview($0, report: report) } ?? true)
                    .accessibilityIdentifier("duplicate-review-selection")
                    .accessibilityHint("Reviews the copy in the group you most recently selected. Your chosen keeper stays in place.")
                }
                Button(model.duplicateReport == nil ? "Check files" : "Check again") {
                    guard !workInProgress else { return }
                    resetChoices(for: model.duplicateReport?.token)
                    model.checkDuplicates()
                }
                .buttonStyle(InkButtonStyle(kind: model.duplicateReport?.groups.isEmpty == false ? .quiet : .primary,
                                           fullWidth: true, compact: true, seed: 927))
                .disabled(workInProgress)
                .accessibilityHint("Reads file contents only for bounded groups of current indexed personal-file suggestions. Changes no files.")
                if model.hasCleanupWork || model.snapshot.cleaning {
                    Text("Finish the current cleanup before checking files.")
                        .font(TeaFont.caption).foregroundStyle(TeaTheme.inkSoft)
                } else if model.snapshot.scanning {
                    Text("Wait for the scan to finish before checking files.")
                        .font(TeaFont.caption).foregroundStyle(TeaTheme.inkSoft)
                } else if model.busy {
                    Text("Wait for the current operation to finish.")
                        .font(TeaFont.caption).foregroundStyle(TeaTheme.inkSoft)
                }
            }
        }
        .padding(.horizontal, TeaTheme.panelPadding)
        .padding(.top, 10).padding(.bottom, 12)
        .background(TeaTheme.paperDeep)
        .overlay(alignment: .top) { InkDivider(seed: 929) }
    }

    private func choicesEnabled(for report: DuplicateReport) -> Bool {
        !workInProgress && currentError == nil && report.expiresInSeconds > 0
            && selectionToken == report.token && model.duplicateReport?.token == report.token
    }

    private func canReview(_ group: DuplicateGroup, report: DuplicateReport) -> Bool {
        guard choicesEnabled(for: report),
              let keeperID = keeperIDs[group.id], let copyID = copyIDs[group.id],
              keeperID != copyID,
              group.files.contains(where: { $0.id == keeperID }),
              let copy = group.files.first(where: { $0.id == copyID }) else { return false }
        return !copy.keeperOnly && copy.candidate.canReviewCleanup
    }

    private func chooseKeeper(_ id: String, group: DuplicateGroup, report: DuplicateReport) {
        guard choicesEnabled(for: report), group.files.contains(where: { $0.id == id }) else { return }
        activeGroupID = group.id
        keeperIDs[group.id] = keeperIDs[group.id] == id ? nil : id
        if copyIDs[group.id] == id { copyIDs[group.id] = nil }
    }

    private func chooseCopy(_ id: String, group: DuplicateGroup, report: DuplicateReport) {
        guard choicesEnabled(for: report), keeperIDs[group.id] != id,
              let file = group.files.first(where: { $0.id == id }),
              !file.keeperOnly, file.candidate.canReviewCleanup else { return }
        activeGroupID = group.id
        copyIDs[group.id] = copyIDs[group.id] == id ? nil : id
    }

    private func reviewSelection(in group: DuplicateGroup, report: DuplicateReport) {
        guard canReview(group, report: report),
              let keeperID = keeperIDs[group.id],
              let copyID = copyIDs[group.id] else { return }
        model.reviewDuplicate(report: report, groupID: group.id,
                              keeperID: keeperID, copyID: copyID)
    }

    private func resetChoices(for token: String?) {
        selectionToken = token
        activeGroupID = nil
        keeperIDs.removeAll(keepingCapacity: false)
        copyIDs.removeAll(keepingCapacity: false)
    }

    private func isPartial(_ report: DuplicateReport) -> Bool {
        !report.progress.complete || report.progress.cancelled || report.progress.limited
            || report.progress.skippedFiles > 0 || report.skippedBuckets > 0
            || report.skippedBucketFiles > 0 || report.bucketLimitReached
    }

    private func reportTitle(_ report: DuplicateReport) -> String {
        let groups = "\(report.groups.count) matching \(report.groups.count == 1 ? "group" : "groups")"
        if report.progress.cancelled { return "Check cancelled · \(groups)" }
        if isPartial(report) { return "Partial check · \(groups)" }
        return report.groups.isEmpty ? "No matches in checked files" : groups
    }

    private func phaseLabel(_ phase: String) -> String {
        phase.isEmpty ? "Checking files…" : phase.replacingOccurrences(of: "_", with: " ").capitalized
    }
}
