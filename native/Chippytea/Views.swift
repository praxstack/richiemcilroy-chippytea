import SwiftUI

// MARK: - Root

/// A page from a pocket notebook, taped up under the menu bar.
struct RootView: View {
    @ObservedObject var model: AppModel
    let updates: UpdateController
    var isPerformanceBenchmark = false
    @Environment(\.accessibilityReduceMotion) private var systemReduceMotion
    @State private var entered = true
    private var reduced: Bool { model.reduceMotion || systemReduceMotion }
    private var boiling: Bool { model.panelVisible && !reduced }

    var body: some View {
        let page = PageShape()
        ZStack(alignment: .topLeading) {
            ZStack(alignment: .top) {
                VStack(spacing: 0) {
                    Masthead(boiling: boiling, presentation: model.presentation)
                        .overlay(alignment: .topTrailing) {
                            if model.hasCleanupWork {
                                CleanupStatusButton(model: model)
                                    .padding(.top, 8).padding(.trailing, TeaTheme.panelPadding)
                            }
                        }
                        .zIndex(1)
                    UpdateBanner(updates: updates)
                    ContentArea(model: model, updates: updates)
                        .modifier(UpdateInteractionGuard(updates: updates))
                    BottomBar(model: model, selectedDestination: model.destination,
                              hasPendingCoins: model.snapshot.wallet.pendingCoins > 0,
                              findingCount: model.displayedCandidates.count)
                        .equatable()
                }
                if model.showReview {
                    ReviewTakeover(model: model)
                        .modifier(UpdateInteractionGuard(updates: updates))
                        .transition(reduced ? AnyTransition.opacity : AnyTransition.move(edge: .trailing).combined(with: .opacity))
                }
                if model.showDiskAccess {
                    DiskAccessView(model: model)
                        .modifier(UpdateInteractionGuard(updates: updates))
                        .transition(.opacity)
                }
            }
            .padding(.top, TeaTheme.tapeOverhang)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background {
                ZStack {
                    page.fill(TeaTheme.paper)
                    DotGrid().clipShape(page)
                }
            }
            .clipShape(page)
            .overlay(page.stroke(TeaTheme.ink.opacity(0.85), style: StrokeStyle(lineWidth: 1.6, lineCap: .round, lineJoin: .round)))
            .opacity(entered ? 1 : 0)
            .scaleEffect(entered ? 1 : 0.985, anchor: .top)
            .offset(y: entered ? 0 : -6)

            Boiling(active: boiling, replay: model.presentation, event: model.collection?.id) { phase in WashiTape(boil: phase) }
                .offset(x: model.anchorX - TeaTheme.tapeWidth / 2, y: 1)
                .opacity(entered ? 1 : 0)
        }
        .frame(width: TeaTheme.panelWidth, height: model.panelHeight)
        .font(TeaFont.body)
        .foregroundStyle(TeaTheme.ink)
        .tint(TeaTheme.biro)
        .animation(reduced ? nil : .spring(response: 0.32, dampingFraction: 0.9), value: model.showReview)
        .animation(reduced ? nil : .easeInOut(duration: 0.15), value: model.showDiskAccess)
        .preferredColorScheme(.light)
        .overlay(alignment: .topTrailing) {
            if isPerformanceBenchmark {
                Text("Performance test · disposable files")
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundStyle(.white)
                    .padding(.horizontal, 10).padding(.vertical, 6)
                    .background(.black.opacity(0.85), in: Capsule())
                    .padding(.top, 28).padding(.trailing, 12)
            }
        }
        // A visible measurement is observational. Its synthetic library must
        // not invite cleanup or collection while the user's real app is closed.
        .allowsHitTesting(!isPerformanceBenchmark)
        .onChange(of: model.presentation) { _, _ in enterPanel() }
        .onReceive(NotificationCenter.default.publisher(for: NSApplication.didBecomeActiveNotification)) { _ in model.diskAccessReturned() }
    }

    private func enterPanel() {
        guard !reduced else { entered = true; return }
        entered = false
        DispatchQueue.main.async {
            withAnimation(.easeOut(duration: 0.18)) { entered = true }
        }
    }
}

/// The shop sign, top left of the page: the battered fish and the hand-lettered
/// wordmark. It sits above every screen; hovering it makes the ink boil.
private struct Masthead: View {
    var boiling: Bool
    var presentation: Int
    @State private var hovering = false

    var body: some View {
        HStack(alignment: .center, spacing: 9) {
            Boiling(active: boiling && hovering, replay: presentation) { phase in
                HStack(alignment: .center, spacing: 9) {
                    BatteredFishLogo(height: 26, boil: phase)
                    HandWordmark(height: 24, boil: phase).padding(.top, 4)
                }
            }
            Spacer(minLength: 0)
        }
        .padding(.horizontal, TeaTheme.panelPadding)
        .padding(.top, 9)
        .onHover { hovering = $0 }
        .accessibilityElement()
        .accessibilityLabel("chippytea")
    }
}

private struct ContentArea: View {
    @ObservedObject var model: AppModel
    let updates: UpdateController
    @Environment(\.accessibilityReduceMotion) private var systemReduceMotion
    private var reduced: Bool { model.reduceMotion || systemReduceMotion }

    var body: some View {
        ZStack(alignment: .top) {
            Group {
                switch model.destination {
                case .coins: CoinsPage(model: model)
                case .discover: DiscoveryPage(model: model)
                case .activity: ActivityPage(model: model)
                case .settings: SettingsPage(model: model, updates: updates)
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
            .transition(.opacity)

            if let error = model.errorMessage {
                ErrorBanner(model: model, message: error)
                    .padding(.horizontal, 12)
                    .padding(.top, 6)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .animation(reduced ? nil : .easeInOut(duration: 0.15), value: model.destination)
    }
}

private struct ErrorBanner: View {
    @ObservedObject var model: AppModel
    let message: String

    var body: some View {
        InkCard(padding: 9, seed: 141, fill: TeaTheme.card, stroke: TeaTheme.rust) {
            HStack(alignment: .top, spacing: 8) {
                Image(systemName: "exclamationmark.circle").font(TeaFont.body).foregroundStyle(TeaTheme.rust)
                Text(message).font(TeaFont.caption).foregroundStyle(TeaTheme.ink)
                    .textSelection(.enabled).fixedSize(horizontal: false, vertical: true)
                Spacer(minLength: 4)
                Button { model.errorMessage = nil } label: {
                    Image(systemName: "xmark").font(TeaFont.caption)
                }
                .buttonStyle(.plain).foregroundStyle(TeaTheme.inkSoft)
                .help("Dismiss message").accessibilityLabel("Dismiss message")
            }
        }
    }
}

private struct CleanupStatusButton: View {
    @ObservedObject var model: AppModel
    @Environment(\.accessibilityReduceMotion) private var systemReduceMotion
    @State private var showDetails = false
    @State private var hovering = false
    private var reduced: Bool { model.reduceMotion || systemReduceMotion }
    private var headline: String {
        model.cleanupCancellationRequested ? "Stopping cleanup…" : model.cleanupProgress?.headline ?? "Cleaning up…"
    }
    private var detail: String {
        model.queuedCleanupCount > 0 ? "\(currentDetail) · \(model.queuedCleanupCount) queued" : currentDetail
    }
    private var currentDetail: String {
        guard let progress = model.cleanupProgress else { return "Your reviewed cleanup is in progress." }
        if progress.itemCount > 1 && (progress.totalEntries > 0 || progress.completedEntries > 0) {
            return "Item \(progress.itemNumber) of \(progress.itemCount) · \(progress.detail)"
        }
        return progress.detail
    }
    private var stopTitle: String {
        if model.cleanupCancellationRequested { return "Stopping…" }
        return model.queuedCleanupCount > 0 ? "Stop all" : "Stop cleanup"
    }

    var body: some View {
        Button { showDetails.toggle() } label: {
            CleanupProgressRing(animated: model.panelVisible && !reduced)
                .frame(width: 28, height: 28)
                .background(WobblyPill(seed: 143).fill(showDetails || hovering ? TeaTheme.paperDeep : .clear))
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
        .help("\(headline) \(detail)")
        .accessibilityLabel("Cleanup in progress")
        .accessibilityValue("\(headline) \(detail)")
        .accessibilityHint("Shows cleanup details and the stop button.")
        .overlay(alignment: .topTrailing) {
            if showDetails {
                details.padding(.top, 34).transition(.opacity)
            }
        }
        .animation(reduced ? nil : .easeOut(duration: 0.12), value: showDetails)
        .onChange(of: model.panelVisible) { _, visible in if !visible { showDetails = false } }
        .onChange(of: model.destination) { _, _ in showDetails = false }
    }

    private var details: some View {
        InkCard(padding: 9, seed: 143) {
            VStack(alignment: .leading, spacing: 6) {
                HStack(spacing: 8) {
                    Text(headline).font(TeaFont.bodySemibold)
                        .lineLimit(1).minimumScaleFactor(0.85)
                    Spacer(minLength: 0)
                    Button { showDetails = false } label: { Image(systemName: "xmark").font(TeaFont.caption) }
                        .buttonStyle(.plain).foregroundStyle(TeaTheme.inkSoft)
                        .accessibilityLabel("Close cleanup details")
                }
                .frame(height: 20)
                Text(detail)
                    .font(TeaFont.caption).monospacedDigit().foregroundStyle(TeaTheme.inkSoft)
                    .lineLimit(1).truncationMode(.middle)
                    .frame(height: 18, alignment: .leading)
                    .help(model.cleanupProgress?.title ?? detail)
                Button(stopTitle) { model.cancel() }
                    .buttonStyle(InkButtonStyle(kind: .quiet, fullWidth: true, compact: true, seed: 145))
                    .disabled(model.cleanupCancellationRequested)
                    .accessibilityHint(model.queuedCleanupCount > 0
                        ? "Stops the current cleanup between filesystem operations and cancels all waiting cleanups. Already completed work remains."
                        : "Stops between filesystem operations; already completed work remains.")
            }
        }
        .frame(width: 260)
    }
}

/// Counts restart between cleanup phases, so this ring deliberately shows activity,
/// not an invented overall percentage. Its timeline exists only while visible.
private struct CleanupProgressRing: View {
    let animated: Bool

    var body: some View {
        Group {
            if animated {
                TimelineView(.animation(minimumInterval: 1.0 / 12.0)) { timeline in
                    ring(rotation: .degrees(timeline.date.timeIntervalSinceReferenceDate.truncatingRemainder(dividingBy: 1.4) / 1.4 * 360))
                }
            } else {
                ring(rotation: .zero)
            }
        }
        .frame(width: 22, height: 22)
        .accessibilityHidden(true)
    }

    private func ring(rotation: Angle) -> some View {
        ZStack {
            ScribbleRing(seed: 143, amplitude: 0.4)
                .stroke(TeaTheme.inkFaint, style: StrokeStyle(lineWidth: 1.4, lineCap: .round))
            ScribbleRing(seed: 143, amplitude: 0.4)
                .trim(from: 0.05, to: 0.65)
                .stroke(TeaTheme.biro, style: StrokeStyle(lineWidth: 1.6, lineCap: .round))
                .rotationEffect(rotation)
        }
    }
}

// MARK: - Bottom bar

private struct BottomBar: View, Equatable {
    // The model is an action target. Rendering depends only on these values,
    // so unrelated scan updates do not invalidate the bar or its tab leaves.
    let model: AppModel
    let selectedDestination: Destination
    let hasPendingCoins: Bool
    let findingCount: Int

    nonisolated static func == (lhs: Self, rhs: Self) -> Bool {
        lhs.model === rhs.model && lhs.selectedDestination == rhs.selectedDestination
            && lhs.hasPendingCoins == rhs.hasPendingCoins && lhs.findingCount == rhs.findingCount
    }

    var body: some View {
        HStack(spacing: 0) {
            TabButton(model: model, destination: .coins, selected: selectedDestination == .coins,
                      hasPendingCoins: hasPendingCoins, findingCount: findingCount, seed: 161)
            TabButton(model: model, destination: .discover, selected: selectedDestination == .discover,
                      hasPendingCoins: hasPendingCoins, findingCount: findingCount, seed: 163)
            TabButton(model: model, destination: .activity, selected: selectedDestination == .activity,
                      hasPendingCoins: hasPendingCoins, findingCount: findingCount, seed: 165)
            WobblyLine(amplitude: 0.6, seed: 167)
                .stroke(TeaTheme.ink.opacity(0.22), style: StrokeStyle(lineWidth: 1.1, lineCap: .round))
                .frame(width: 22, height: 2)
                .rotationEffect(.degrees(90))
                .frame(width: 12)
            TabButton(model: model, destination: .settings, selected: selectedDestination == .settings,
                      hasPendingCoins: hasPendingCoins, findingCount: findingCount, seed: 169, iconOnly: true)
        }
        .padding(.horizontal, 10).padding(.top, 4).padding(.bottom, 5)
        .background(TeaTheme.paperDeep.opacity(0.55))
        .overlay(alignment: .top) {
            WobblyLine(amplitude: 0.8, seed: 171)
                .stroke(TeaTheme.ink.opacity(0.3), style: StrokeStyle(lineWidth: 1.2, lineCap: .round))
                .frame(height: 3)
        }
    }
}

private struct TabButton: View {
    let model: AppModel
    let destination: Destination
    let selected: Bool
    let hasPendingCoins: Bool
    let findingCount: Int
    var seed: Int
    var iconOnly = false
    @State private var hovering = false

    private var accessibleName: String {
        if destination == .coins && hasPendingCoins { return "Your chips, chips ready to collect" }
        if destination == .discover && findingCount > 0 { return "\(destination.rawValue), \(findingCount) findings" }
        return destination.rawValue
    }

    var body: some View {
        Button {
            model.destination = destination
            if destination == .coins { model.collect() }
        } label: {
            VStack(spacing: 2) {
                ZStack {
                    if selected {
                        ScribbleRing(seed: seed &+ 7)
                            .stroke(TeaTheme.biro, style: StrokeStyle(lineWidth: 1.4, lineCap: .round))
                            .frame(width: 28, height: 25)
                    }
                    if destination == .coins {
                        // The tab icon is the chip bundle itself: outlined at rest, golden when here.
                        ChipsDoodle(size: 17, filled: selected, seed: 23)
                    } else {
                        Image(systemName: destination.symbol)
                            .font(.system(size: 13, weight: selected ? .semibold : .regular, design: .rounded))
                            .frame(height: 17)
                    }
                    if destination == .coins && hasPendingCoins {
                        Circle().fill(TeaTheme.gold)
                            .overlay(Circle().stroke(TeaTheme.ink.opacity(0.7), lineWidth: 0.9))
                            .frame(width: 6, height: 6).offset(x: 13, y: -8)
                    }
                    if destination == .discover && findingCount > 0 {
                        Text(findingCount.formatted())
                            .font(TeaFont.captionSemibold).foregroundStyle(TeaTheme.ink)
                            .padding(.horizontal, 4).padding(.vertical, 0.5)
                            .background(WobblyPill(seed: seed &+ 11).fill(TeaTheme.gold.opacity(0.85)))
                            .overlay(WobblyPill(seed: seed &+ 11).stroke(TeaTheme.ink.opacity(0.55), lineWidth: 1))
                            .offset(x: 19, y: -9)
                    }
                }
                .frame(width: iconOnly ? 30 : nil, height: 26)
                if !iconOnly {
                    Text(destination.tabTitle).font(TeaFont.caption).lineLimit(1)
                }
            }
            .foregroundStyle(selected ? TeaTheme.biro : TeaTheme.inkSoft)
            .frame(maxWidth: iconOnly ? nil : .infinity)
            .padding(.horizontal, iconOnly ? 8 : 2)
            .padding(.top, 5).padding(.bottom, 3)
            .background(hovering && !selected ? TeaTheme.paperDeep.opacity(0.8) : .clear)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
        .accessibilityAddTraits(selected ? [.isSelected] : [])
        .accessibilityLabel(accessibleName)
        .help(destination.rawValue)
    }
}

// MARK: - Home

/// Home is about the space: what chippytea has saved, what is free, and what
/// to clean next. The chips sit underneath, the reward for all of it.
private struct CoinsPage: View {
    @ObservedObject var model: AppModel
    /// Home only ever offers what the engine actually recommends.
    private var suggestions: [Candidate] { Array(model.displayedCandidates.filter(\.recommended).prefix(3)) }
    private var emptyMessage: String {
        if model.hasCleanupWork { return "Your cleanup is in progress." }
        return model.discoveryPresentation.isForeground ? "Having a look through your folders…" : "Nothing to clean up. Stick the kettle on."
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 0) {
                VStack(alignment: .leading, spacing: 8) {
                    SavedHeadline(model: model)
                    if let storage = model.storageStatus {
                        StorageStrip(model: model, status: storage)
                    }
                    InkDivider(seed: 179)
                    ScreenTitle(text: "Make a bit of room.", seed: 181, font: TeaFont.subtitle)
                    nextSteps
                    InkDivider(seed: 185).padding(.top, 4)
                }
                .padding(.horizontal, TeaTheme.panelPadding)
                .padding(.top, 8)
                ChipsStrip(model: model)
            }
            .coordinateSpace(name: "home")
        }
        .scrollIndicators(.hidden)
    }

    @ViewBuilder private var nextSteps: some View {
        if model.snapshot.roots.isEmpty {
            ScanEverywhereCTA(model: model, seed: 183)
        } else if !suggestions.isEmpty {
            InkCard(padding: 0, seed: 187) {
                VStack(spacing: 0) {
                    ForEach(Array(suggestions.enumerated()), id: \.element.id) { index, item in
                        SuggestionRow(model: model, candidate: item, seed: 221 + index * 8)
                        if index < suggestions.count - 1 {
                            InkDivider(seed: 189 + index * 4).padding(.horizontal, 10)
                        }
                    }
                }
            }
        } else {
            InkCard(seed: 191) {
                HStack(spacing: 10) {
                    if model.discoveryPresentation.isForeground { MagnifierDoodle(size: 32) } else { MugDoodle(size: 32) }
                    Text(emptyMessage)
                        .font(TeaFont.bodySemibold).fixedSize(horizontal: false, vertical: true)
                    Spacer(minLength: 0)
                }
            }
        }
    }
}

/// The headline of the whole app: space freed for good, measured after each
/// reviewed cleanup, never estimated. Moving files to Trash is not counted.
private struct SavedHeadline: View {
    @ObservedObject var model: AppModel
    private var saved: UInt64 { model.snapshot.wallet.creditedBytes }

    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            ScreenTitle(text: saved == 0 ? "No space saved yet." : "\(space(saved)) saved.", seed: 175, font: TeaFont.headline)
                .monospacedDigit()
            Text(saved == 0
                 ? "Your first reviewed cleanup lands here."
                 : "Freed for good by cleanups you reviewed, measured after each one.")
                .font(TeaFont.caption).foregroundStyle(TeaTheme.inkSoft)
                .fixedSize(horizontal: false, vertical: true)
        }
        .accessibilityElement(children: .combine)
    }
}

private struct StripBottomKey: PreferenceKey {
    static let defaultValue: CGFloat = 0
    static func reduce(value: inout CGFloat, nextValue: () -> CGFloat) { value = max(value, nextValue()) }
}

private struct WrapFrameKey: PreferenceKey {
    static let defaultValue: CGRect = .zero
    /// Siblings without the key reduce their default in; only the wrap's frame counts.
    static func reduce(value: inout CGRect, nextValue: () -> CGRect) {
        let next = nextValue()
        if next != .zero { value = next }
    }
}

/// Your chips: the wrap beside the hand-lettered count, and the moment they
/// tumble in from under the tape. Counting and collection state live here.
private struct ChipsStrip: View {
    @ObservedObject var model: AppModel
    @Environment(\.accessibilityReduceMotion) private var systemReduceMotion
    @State private var counter: Double = 0
    @State private var landing = false
    @State private var showHowChipsWork = false
    @State private var presentedCollection: CollectionBurst?
    @State private var previewBurst: CleanupPreview?
    @State private var stripBottom: CGFloat = 0
    @State private var wrapFrame: CGRect = .zero
    private var wallet: Wallet { model.snapshot.wallet }
    private var reduced: Bool { model.reduceMotion || systemReduceMotion }
    private var empty: Bool { model.displayedCoinBalance == 0 }
    private var contentVisible: Bool { model.panelVisible && !model.showReview && !model.showDiskAccess }
    private var boiling: Bool { contentVisible && !reduced }
    private var remainingBytes: UInt64 { wallet.fractionalBytes >= 100_000_000 ? 0 : 100_000_000 - wallet.fractionalBytes }
    private var caption: String {
        if model.hasCleanupWork {
            if !model.pendingCleanupIncludesPermanent { return "Moving to Trash…" }
            let estimate = model.pendingCleanupCoinEstimate
            if estimate == 0 { return "Space credit pending" }
            return "Up to \(chipsPhrase(estimate)) pending"
        }
        return empty ? "Your first chip’s still in the fryer." : "\(space(remainingBytes)) to your next chip"
    }
    private var balanceAccessibilityLabel: String {
        if model.hasCleanupWork && model.pendingCleanupCoinEstimate > 0 {
            return "\(chipsPhrase(model.confirmedEarnedCoins)) confirmed, up to \(chipsPhrase(model.pendingCleanupCoinEstimate)) pending cleanup"
        }
        return "\(chipsPhrase(model.displayedCoinBalance)) in the paper"
    }

    private struct CollectionAnimation: Hashable {
        let id: UUID?
        let reduced: Bool
        let visible: Bool
        var target: UInt64? = nil
    }

    var body: some View {
        let animation = CollectionAnimation(id: model.collection?.id, reduced: reduced, visible: contentVisible)
        let previewAnimation = CollectionAnimation(id: model.cleanupPreview?.id, reduced: reduced,
                                                   visible: contentVisible && model.collection == nil,
                                                   target: model.cleanupPreview?.targetCoins)
        let activeBurstID = model.collection?.id ?? previewBurst?.id
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 6) {
                ScreenTitle(text: "Your chips.", seed: 199, font: TeaFont.subtitle)
                Button {
                    withAnimation(reduced ? nil : .easeInOut(duration: 0.15)) { showHowChipsWork.toggle() }
                } label: {
                    Image(systemName: showHowChipsWork ? "info.circle.fill" : "info.circle")
                        .font(TeaFont.caption)
                        .foregroundStyle(showHowChipsWork ? TeaTheme.biro : TeaTheme.inkSoft)
                }
                .buttonStyle(.plain)
                .help("How chips work")
                .accessibilityLabel("How chips work")
                Spacer(minLength: 0)
            }
            HStack(alignment: .center, spacing: 12) {
                Boiling(active: boiling, replay: model.presentation, event: activeBurstID) { phase in
                    ChipPortion(chips: model.displayedCoinBalance, landing: boiling && landing, boil: phase, scale: 0.5)
                        .frame(width: 156, height: 64)
                        .id(boiling)
                }
                .background(GeometryReader { geometry in
                    Color.clear.preference(key: WrapFrameKey.self, value: geometry.frame(in: .named("home")))
                })
                VStack(alignment: .leading, spacing: 5) {
                    Boiling(active: boiling && activeBurstID != nil, event: activeBurstID) { phase in
                        AnimatedChipNumber(value: counter, digitHeight: 28, boil: phase)
                            // A new identity discards in-flight interpolation
                            // when hidden, motion stops, or a verified target changes.
                            .id(previewAnimation)
                    }
                    .accessibilityElement()
                    .accessibilityLabel(balanceAccessibilityLabel)
                    Text(caption)
                        .font(TeaFont.caption).monospacedDigit().foregroundStyle(TeaTheme.inkSoft)
                        .lineLimit(1).minimumScaleFactor(0.8)
                    ChipProgressMeter(fraction: Double(wallet.fractionalBytes) / 100_000_000)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            collectAction
            if showHowChipsWork {
                HowChipsWorkSlip {
                    withAnimation(reduced ? nil : .easeInOut(duration: 0.15)) { showHowChipsWork = false }
                }
                .transition(reduced ? .opacity : .opacity.combined(with: .move(edge: .top)))
            }
        }
        .padding(.horizontal, TeaTheme.panelPadding)
        .padding(.top, 10).padding(.bottom, 15)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(GeometryReader { geometry in
            Color.clear.preference(key: StripBottomKey.self, value: geometry.frame(in: .named("home")).maxY)
        })
        .onPreferenceChange(WrapFrameKey.self) { wrapFrame = $0 }
        .onPreferenceChange(StripBottomKey.self) { stripBottom = $0 }
        .overlay(alignment: .bottom) {
            // The chips tumble in from under the tape: the overlay reaches back
            // up to the top of the page and lands them in the wrap's heap.
            burst.frame(height: max(stripBottom, 1))
        }
        .onAppear { counter = Double(model.collection?.from ?? model.cleanupPreview?.from ?? model.displayedCoinBalance) }
        .onDisappear {
            if let id = presentedCollection?.id { model.finishCollection(id: id) }
            if let id = previewAnimation.id { model.finishCleanupPreview(id) }
        }
        .onChange(of: model.displayedCoinBalance) { _, value in
            if model.collection == nil && model.cleanupPreview == nil { settleBalance(value) }
        }
        .task(id: animation) {
            guard !Task.isCancelled, model.collection?.id == animation.id else { return }
            guard let burst = model.collection else {
                // A pending cleanup owns the counter until its verified result
                // settles. Only the model retains confirmed collection floors.
                if model.cleanupPreview == nil { settleBalance(model.displayedCoinBalance) }
                return
            }
            let alreadyPresented = presentedCollection?.id == burst.id
            if !alreadyPresented { presentedCollection = burst }
            guard boiling, !alreadyPresented else {
                settleCollection(burst)
                return
            }
            counter = Double(burst.from)
            do {
                try await Task.sleep(for: .milliseconds(30))
                guard !Task.isCancelled, model.collection?.id == burst.id, boiling else { return }
                withAnimation(.easeOut(duration: 1.35)) { counter = Double(burst.to) }
                try await Task.sleep(for: .milliseconds(780))
                guard !Task.isCancelled, model.collection?.id == burst.id, boiling else { return }
                withAnimation(.spring(response: 0.2, dampingFraction: 0.45)) { landing = true }
                try await Task.sleep(for: .milliseconds(170))
                guard !Task.isCancelled, model.collection?.id == burst.id, boiling else { return }
                withAnimation(.spring(response: 0.34, dampingFraction: 0.45)) { landing = false }
                try await Task.sleep(for: .milliseconds(800))
                guard !Task.isCancelled, model.collection?.id == burst.id else { return }
                model.finishCollection(id: burst.id)
            } catch {
                // The rekeyed task settles the current presentation. Leaving
                // state alone here prevents a cancelled burst altering its successor.
            }
        }
        .task(id: previewAnimation) {
            guard !Task.isCancelled, model.cleanupPreview?.id == previewAnimation.id,
                  model.cleanupPreview?.targetCoins == previewAnimation.target else { return }
            previewBurst = nil
            guard let preview = model.cleanupPreview else {
                if model.collection == nil { settleBalance(model.displayedCoinBalance) }
                return
            }
            guard contentVisible else {
                settlePreview(preview.id)
                return
            }
            // Let an earlier earned collection finish before this attempt gets
            // the counter. Hiding the panel still settles both presentations.
            guard model.collection == nil else { return }
            guard model.claimCleanupPreview(preview.id) else {
                settlePreview(preview.id)
                return
            }
            // Static feedback is consumed too; restoring motion or reopening
            // must never replay the confirmation animation or its sound.
            guard !reduced, preview.isPermanent, preview.targetCoins > preview.from else {
                settlePreview(preview.id)
                return
            }
            previewBurst = preview
            counter = Double(preview.from)
            do {
                try await Task.sleep(for: .milliseconds(30))
                guard !Task.isCancelled, previewIsCurrent(preview), boiling else { return }
                withAnimation(.easeOut(duration: 1.35)) { counter = Double(preview.targetCoins) }
                try await Task.sleep(for: .milliseconds(780))
                guard !Task.isCancelled, previewIsCurrent(preview), boiling else { return }
                withAnimation(.spring(response: 0.2, dampingFraction: 0.45)) { landing = true }
                try await Task.sleep(for: .milliseconds(170))
                guard !Task.isCancelled, previewIsCurrent(preview), boiling else { return }
                withAnimation(.spring(response: 0.34, dampingFraction: 0.45)) { landing = false }
                try await Task.sleep(for: .milliseconds(800))
                guard !Task.isCancelled, previewIsCurrent(preview) else { return }
                previewBurst = nil
                model.finishCleanupPreview(preview.id)
            } catch {
                // The replacement task settles a changed verified target. A
                // cancelled estimate cannot restore its captured balance.
            }
        }
    }

    /// The burst lands in the heap of the wrap; the "+N" scrawl pops in on the
    /// title row, to the right of "Your chips."
    @ViewBuilder private var burst: some View {
        let landing = CGRect(x: wrapFrame.minX + 40, y: wrapFrame.minY + 22, width: 76, height: 16)
        let tally = CGPoint(x: TeaTheme.panelWidth - 92, y: max(0, wrapFrame.minY - 32))
        if let burst = model.collection {
            if boiling && burst.showsParticles {
                ChipCollectionOverlay(amount: burst.amount, anchorX: model.anchorX, landing: landing, tallyOrigin: tally)
                    .id(burst.id)
            }
        } else if let preview = previewBurst, preview.id == model.cleanupPreview?.id,
                  preview.targetCoins == model.cleanupPreview?.targetCoins,
                  model.cleanupPreview?.presentationFinished == false,
                  preview.targetCoins > preview.from, boiling {
            ChipCollectionOverlay(amount: preview.targetCoins - preview.from, anchorX: model.anchorX, landing: landing, tallyOrigin: tally)
                .id(preview.id)
        }
    }

    private func settleCollection(_ burst: CollectionBurst) {
        settleBalance(model.displayedCoinBalance)
        model.finishCollection(id: burst.id)
    }

    private func previewIsCurrent(_ preview: CleanupPreview) -> Bool {
        model.collection == nil && model.cleanupPreview?.id == preview.id
            && model.cleanupPreview?.targetCoins == preview.targetCoins
            && model.cleanupPreview?.presentationFinished == false
    }

    private func settlePreview(_ id: UUID) {
        settleBalance(model.displayedCoinBalance)
        model.finishCleanupPreview(id)
    }

    private func settleBalance(_ balance: UInt64) {
        var transaction = Transaction(animation: nil)
        transaction.disablesAnimations = true
        withTransaction(transaction) {
            counter = Double(balance)
            landing = false
        }
    }

    @ViewBuilder private var collectAction: some View {
        if wallet.pendingCoins > 0 && model.cleanupPreview == nil
            && model.confirmedCollectedCoins <= wallet.collectedCoins {
            Button("Collect \(chipsPhrase(wallet.pendingCoins))") { model.collect() }
                .buttonStyle(InkButtonStyle(kind: .primary, fullWidth: true, compact: true, seed: 205))
                .disabled(model.busy || model.collection != nil)
                .accessibilityHint("Adds chips you have already earned to your paper. Does not remove any files.")
                .padding(.top, 4)
        }
    }
}

/// The room left on this Mac: a mug of tea whose level is the free space,
/// poured freshly each time the panel opens, with the honest numbers beside it.
private struct StorageStrip: View {
    @ObservedObject var model: AppModel
    let status: StorageStatus
    @Environment(\.accessibilityReduceMotion) private var systemReduceMotion
    @State private var poured = false
    private var reduced: Bool { model.reduceMotion || systemReduceMotion }
    private var fraction: Double {
        status.totalBytes == 0 ? 0 : Double(status.availableBytes) / Double(status.totalBytes)
    }
    private var flavour: String {
        if fraction > 0.5 { return "plenty of room in the pot" }
        if fraction > 0.2 { return "room for a good few chips yet" }
        if fraction > 0.08 { return "getting cosy in here" }
        return "nearly full — time for a tidy"
    }

    var body: some View {
        HStack(spacing: 11) {
            Boiling(active: model.panelVisible && !reduced, replay: model.presentation) { phase in
                StorageMug(fraction: poured ? fraction : 0, size: 44, boil: phase)
            }
            VStack(alignment: .leading, spacing: 1) {
                Text("\(space(status.availableBytes)) free on this Mac")
                    .font(TeaFont.bodySemibold).monospacedDigit()
                Text("of \(space(status.totalBytes)) · \(flavour)")
                    .font(TeaFont.caption).foregroundStyle(TeaTheme.inkSoft)
                    .lineLimit(1).minimumScaleFactor(0.85)
            }
            Spacer(minLength: 0)
        }
        .help("The startup disk’s available space, as macOS reports it. Other volumes and reserved space are counted in the total.")
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("\(space(status.availableBytes)) free of \(space(status.totalBytes)) on this Mac.")
        .onAppear { pour() }
        .onChange(of: model.presentation) { _, _ in pour() }
    }

    /// The pour re-runs per presentation; Reduce Motion serves the mug settled.
    private func pour() {
        guard !reduced, model.panelVisible else { poured = true; return }
        poured = false
        DispatchQueue.main.async {
            withAnimation(.spring(response: 0.85, dampingFraction: 0.85).delay(0.12)) { poured = true }
        }
    }
}

/// The slip of paper the (i) opens: how space becomes chips, in four honest lines.
private struct HowChipsWorkSlip: View {
    let dismiss: () -> Void

    var body: some View {
        InkCard(padding: 10, seed: 209) {
            VStack(alignment: .leading, spacing: 6) {
                HStack(spacing: 7) {
                    ChipsDoodle(size: 15, seed: 43)
                    Text("How chips work").font(TeaFont.bodySemibold)
                    Spacer(minLength: 4)
                    Button { dismiss() } label: { Image(systemName: "xmark").font(TeaFont.caption) }
                        .buttonStyle(.plain).foregroundStyle(TeaTheme.inkSoft)
                        .help("Close").accessibilityLabel("Close how chips work")
                }
                line("Clean something up for good and the freed space is measured conservatively, then counted as saved.", seed: 211)
                line("Every 100 MB saved earns one chip. Anything smaller is scraps — carried forward, never lost.", seed: 213)
                line("Moving files to Trash earns no chips; nothing is freed until Trash empties.", seed: 215)
                line("Chips stay on your Mac and have no monetary value. They’re just your tea.", seed: 217)
            }
        }
        .padding(.top, 8)
    }

    private func line(_ text: String, seed: Int) -> some View {
        HStack(alignment: .top, spacing: 7) {
            WobblyLine(amplitude: 0.5, seed: seed)
                .stroke(TeaTheme.goldDeep, style: StrokeStyle(lineWidth: 2, lineCap: .round))
                .frame(width: 8, height: 2)
                .padding(.top, 5)
            Text(text).font(TeaFont.caption).foregroundStyle(TeaTheme.inkSoft)
                .fixedSize(horizontal: false, vertical: true).lineSpacing(2)
        }
    }
}

/// The whole "where may I look" decision, in one gold button and one quiet link.
private struct ScanEverywhereCTA: View {
    @ObservedObject var model: AppModel
    var seed: Int
    private var changing: Bool { model.busy || model.scanActivityForUI }

    var body: some View {
        VStack(alignment: .leading, spacing: 7) {
            Button("Scan my Mac") { model.scanMyMac() }
                .buttonStyle(InkButtonStyle(kind: .primary, fullWidth: true, seed: seed))
                .disabled(changing)
                .accessibilityHint("Looks through your home folder for app caches, old logs, build files and large personal files to review. Nothing is selected or removed automatically.")
            Menu {
                Button("Projects Folder…") { model.chooseFolder(kind: "projects") }
                Button("Downloads Folder…") { model.chooseFolder(kind: "downloads") }
                Button("Another Folder…") { model.chooseFolder(kind: "folder") }
            } label: {
                Text("Choose specific folders instead…").font(TeaFont.captionMedium)
            }
            .menuStyle(.borderlessButton).menuIndicator(.hidden).fixedSize()
            .foregroundStyle(TeaTheme.biro)
            .disabled(changing)
            .accessibilityLabel("Choose specific folders instead")
            Text("Set up scan access first. Nothing is removed without your review.")
                .font(TeaFont.caption).foregroundStyle(TeaTheme.inkSoft)
                .lineSpacing(2).fixedSize(horizontal: false, vertical: true)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

/// Home and discovery share the same deletion confirmation preference.
private struct SuggestionRow: View {
    @ObservedObject var model: AppModel
    let candidate: Candidate
    var seed: Int

    private var live: Candidate? { model.displayedCandidates.first { $0.id == candidate.id } }
    private var current: Bool {
        guard let live else { return false }
        return live == candidate && model.snapshot.roots.contains { $0.id == live.rootId }
    }
    private var mayReview: Bool { model.canEnqueueCleanup && current && live?.canReviewCleanup == true }
    private var deletesWithoutConfirmation: Bool { !model.confirmBeforeDeleting && permanentCleanupEligible(candidate) }

    var body: some View {
        HStack(spacing: 9) {
            ArtifactIcon(candidate: candidate, size: 26, seed: seed)
            VStack(alignment: .leading, spacing: 2) {
                Text(candidate.displayName).font(TeaFont.bodySemibold).lineLimit(1).truncationMode(.middle)
                Text(candidate.category).font(TeaFont.caption).foregroundStyle(TeaTheme.inkSoft).lineLimit(1)
                Text(candidate.path).font(TeaFont.mono).foregroundStyle(TeaTheme.inkSoft)
                    .lineLimit(1).truncationMode(.head).help(candidate.path)
            }
            Spacer(minLength: 4)
            Text(space(candidate.allocatedBytes)).font(TeaFont.bodyNumber).monospacedDigit().fixedSize()
            Button(deletesWithoutConfirmation ? "Delete" : "Clean up") { if mayReview { model.requestCleanupOne(candidate) } }
                .buttonStyle(InkButtonStyle(kind: deletesWithoutConfirmation ? .destructive : .quiet, compact: true, seed: seed &+ 3))
                .disabled(!mayReview)
                .accessibilityLabel(deletesWithoutConfirmation
                    ? "Delete \(candidate.displayName) permanently. Path: \(candidate.path)"
                    : "Review cleanup for \(candidate.displayName). Path: \(candidate.path)")
                .accessibilityHint(deletesWithoutConfirmation
                    ? model.hasCleanupWork
                        ? "Queues this developer artifact for permanent deletion after the current cleanup. No further prompt, Trash or restore."
                        : "Permanently deletes this developer artifact without another prompt. No Trash or restore."
                    : permanentCleanupEligible(candidate)
                        ? "Shows the exact item and consequences before you confirm permanent deletion."
                        : "Shows the exact item and consequences before you choose whether to move it to Trash.")
        }
        .help("\(candidate.displayName) · \(candidate.category)\n\(candidate.path)\n\(space(candidate.allocatedBytes)) estimated")
        .padding(.horizontal, 11).padding(.vertical, 8)
    }
}

private struct ArtifactIcon: View {
    let candidate: Candidate
    var size: CGFloat = 30
    var seed: Int = 231
    private var developer: Bool { candidate.isDeveloper }

    var body: some View {
        let shape = WobblyRect(radius: 8, amplitude: 0.7, seed: seed, step: 7)
        Image(systemName: candidate.symbol)
            .font(.system(size: size * 0.44, weight: .medium, design: .rounded))
            .foregroundStyle(TeaTheme.ink)
            .frame(width: size, height: size)
            .background(shape.fill(developer ? TeaTheme.biro.opacity(0.10) : TeaTheme.gold.opacity(0.22)))
            .overlay(shape.stroke(TeaTheme.ink.opacity(0.4), lineWidth: 1.1))
            .accessibilityHidden(true)
    }
}

// MARK: - Find space

private struct DiscoveryPage: View {
    @ObservedObject var model: AppModel
    @State private var filter: DiscoveryFilter = .all
    @State private var sort: DiscoverySort = .suggested
    @State private var search = ""

    private var changing: Bool { model.busy || model.snapshot.cleaning || model.scanActivityForUI }
    private var showingInitialFindings: Bool { model.discoveryPresentation.isForeground && model.displayedCandidates.isEmpty }
    private var emptyTitle: String {
        if model.hasCleanupWork { return "Working on your cleanup…" }
        return showingInitialFindings ? "Having a nosey…" : "Nothing to review here"
    }
    private var emptyDetail: String {
        if model.hasCleanupWork { return "Cleanup continues in the background. Results will appear in Activity." }
        if showingInitialFindings { return "Findings appear as your folders are scanned." }
        if filter == .all && search.isEmpty {
            return "Nothing meets the cleanup checks yet. Small, recently changed or unverified items are left alone."
        }
        return "No supported candidates match this view. Excluded locations stay untouched."
    }

    private var candidates: [Candidate] {
        sort.ordered(model.displayedCandidates.filter { item in
            filter.matches(item) && item.matchesSearch(search)
        })
    }

    private var selectedItems: [Candidate] { model.displayedCandidates.filter { model.selection.contains($0.id) } }
    private var selectedBytes: UInt64 { selectedItems.reduce(0) { $0 &+ $1.allocatedBytes } }
    private var selectionIsCurrent: Bool {
        !selectedItems.isEmpty && selectedItems.count == model.selection.count
            && selectedItems.allSatisfy(\.canReviewCleanup)
    }
    private var deletesWithoutConfirmation: Bool {
        !model.confirmBeforeDeleting && !selectedItems.isEmpty && selectedItems.allSatisfy(permanentCleanupEligible)
    }
    private var selectionDetail: String {
        let limit = model.selection.count >= 100 ? "Up to 100 items · " : ""
        if !selectedItems.isEmpty && selectedItems.allSatisfy(permanentCleanupEligible) {
            return limit + (model.confirmBeforeDeleting ? "Permanent cleanup · confirmation required" : "Permanent cleanup · no Trash or restore")
        }
        return limit + "Trash only for this selection"
    }

    var body: some View {
        let visibleCandidates = candidates
        VStack(spacing: 0) {
            header
            if model.snapshot.roots.isEmpty {
                FolderEmptyState(model: model)
            } else {
                ScanStatusLine(model: model).padding(.horizontal, TeaTheme.panelPadding).padding(.bottom, 7)
                tools(candidateCount: visibleCandidates.count).padding(.horizontal, TeaTheme.panelPadding).padding(.bottom, 8)
                ZStack(alignment: .bottom) {
                    ScrollView {
                        LazyVStack(spacing: 8) {
                            if visibleCandidates.isEmpty {
                                EmptyState(doodle: .magnifier,
                                           title: emptyTitle,
                                           detail: emptyDetail)
                                    .padding(.top, 14)
                            } else {
                                ForEach(Array(visibleCandidates.enumerated()), id: \.element.id) { index, item in
                                    CandidateRow(model: model, candidate: item, seed: 241 + index * 6)
                                }
                            }
                        }
                        .padding(.horizontal, TeaTheme.panelPadding)
                        .padding(.top, 2)
                        .padding(.bottom, model.selection.isEmpty ? 12 : 128)
                    }
                    .scrollIndicators(.hidden)

                    if !model.selection.isEmpty { selectionBar }
                }
                .animation(.spring(response: 0.30, dampingFraction: 0.88), value: model.selection.isEmpty)
            }
        }
    }

    private var header: some View {
        HStack(spacing: 6) {
            ScreenTitle(text: "Find space", seed: 251)
            Spacer(minLength: 0)
            Button { model.refresh() } label: { Image(systemName: "arrow.clockwise") }
                .buttonStyle(InkIconButtonStyle(seed: 253))
                .disabled(model.busy || model.snapshot.cleaning || model.discoveryPresentation.isForeground
                          || model.discoveryPresentation.isRequestPending)
                .help("Refresh authorised folders").accessibilityLabel("Refresh authorised folders")
            Menu {
                Button("Choose Projects Folder…") { model.chooseFolder(kind: "projects") }
                Button("Choose Downloads Folder…") { model.chooseFolder(kind: "downloads") }
                Button("Choose Another Folder…") { model.chooseFolder(kind: "folder") }
            } label: {
                Image(systemName: "folder.badge.plus").inkMenuChrome(seed: 255)
            }
            .menuStyle(.borderlessButton).menuIndicator(.hidden).fixedSize()
            .foregroundStyle(TeaTheme.ink)
            .disabled(changing)
            .help("Add a folder chippytea may look inside").accessibilityLabel("Add a folder")
        }
        .padding(.horizontal, TeaTheme.panelPadding)
        .padding(.top, 8).padding(.bottom, 8)
    }

    private func tools(candidateCount: Int) -> some View {
        VStack(spacing: 7) {
            HStack(spacing: 6) {
                ForEach(Array(DiscoveryFilter.allCases.enumerated()), id: \.element) { index, item in
                    Button { filter = item } label: { Text(item.rawValue).lineLimit(1).fixedSize() }
                        .buttonStyle(ChipButtonStyle(active: filter == item, seed: 261 + index * 4))
                        .accessibilityAddTraits(filter == item ? [.isSelected] : [])
                }
                Spacer(minLength: 0)
            }
            HStack(spacing: 7) {
                Image(systemName: "magnifyingglass").font(TeaFont.caption).foregroundStyle(TeaTheme.inkSoft)
                TextField("Filter by name or path", text: $search)
                    .textFieldStyle(.plain).font(TeaFont.body).foregroundStyle(TeaTheme.ink)
                    .accessibilityLabel("Filter findings by name or path")
                if !search.isEmpty {
                    Button { search = "" } label: { Image(systemName: "xmark.circle.fill").font(TeaFont.caption) }
                        .buttonStyle(.plain).foregroundStyle(TeaTheme.inkSoft)
                        .accessibilityLabel("Clear filter")
                }
                Text(candidateCount.formatted())
                    .font(TeaFont.caption).monospacedDigit().foregroundStyle(TeaTheme.inkSoft)
                    .help("\(candidateCount) findings in this view")
                Menu {
                    Picker("Sort", selection: $sort) {
                        ForEach(DiscoverySort.allCases, id: \.self) { Text($0.rawValue).tag($0) }
                    }
                    .pickerStyle(.inline).labelsHidden()
                } label: {
                    Image(systemName: "arrow.up.arrow.down").inkMenuChrome(seed: 273)
                }
                .menuStyle(.borderlessButton).menuIndicator(.hidden).fixedSize()
                .foregroundStyle(TeaTheme.ink)
                .help("Sort findings — \(sort.rawValue)").accessibilityLabel("Sort findings — \(sort.rawValue)")
            }
            .padding(.horizontal, 10).padding(.vertical, 7)
            .background(WobblyRect(radius: TeaTheme.controlRadius, amplitude: 0.8, seed: 275, step: 10).fill(TeaTheme.card))
            .overlay(WobblyRect(radius: TeaTheme.controlRadius, amplitude: 0.8, seed: 275, step: 10)
                .stroke(TeaTheme.ink.opacity(0.55), style: StrokeStyle(lineWidth: 1.2, lineCap: .round, lineJoin: .round)))
        }
    }

    private var selectionBar: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text("\(model.selection.count) selected · \(space(selectedBytes)) estimated")
                    .font(TeaFont.bodySemibold).monospacedDigit()
                Spacer(minLength: 4)
                Button("Clear") { model.selection.removeAll() }
                    .buttonStyle(InkButtonStyle(kind: .quiet, compact: true, seed: 281))
                    .accessibilityLabel("Clear selection")
            }
            Text(selectionDetail)
                .font(TeaFont.caption).foregroundStyle(TeaTheme.inkSoft)
                .fixedSize(horizontal: false, vertical: true)
            Button { model.requestCleanupSelection() } label: {
                HStack(spacing: 7) {
                    Text(deletesWithoutConfirmation ? "Delete permanently" : "Review cleanup")
                    Image(systemName: deletesWithoutConfirmation ? "trash" : "arrow.right").font(TeaFont.caption)
                }
            }
            .buttonStyle(InkButtonStyle(kind: deletesWithoutConfirmation ? .destructive : .primary, fullWidth: true, seed: 283))
            .disabled(!model.canEnqueueCleanup || !selectionIsCurrent)
            .accessibilityHint(deletesWithoutConfirmation
                ? model.hasCleanupWork
                    ? "Queues the selected developer artifacts for permanent deletion after the current cleanup. No further prompt, Trash or restore."
                    : "Permanently deletes the selected developer artifacts without another prompt. No Trash or restore."
                : "Shows the exact selected items and consequences before cleanup.")
        }
        .padding(.horizontal, 16).padding(.top, 8).padding(.bottom, 10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(TeaTheme.paperDeep)
        .overlay(alignment: .top) {
            WobblyLine(amplitude: 0.8, seed: 285)
                .stroke(TeaTheme.ink.opacity(0.35), style: StrokeStyle(lineWidth: 1.2, lineCap: .round))
                .frame(height: 3)
        }
        .transition(.move(edge: .bottom).combined(with: .opacity))
    }
}

private struct ScanStatusLine: View {
    @ObservedObject var model: AppModel
    private var stats: ScanStats { model.discoveryPresentation.stats }
    private var headline: String {
        if model.hasCleanupWork { return model.snapshot.scanning ? "Discovery paused during cleanup" : "Cleanup in progress" }
        if model.discoveryPresentation.isRequestPending { return "Starting scan…" }
        if model.discoveryPresentation.isForeground { return "Having a look through your folders…" }
        if model.backgroundActivityVisible { return "Updating changed folders in the background" }
        if stats.cancelled { return "Scan cancelled · partial results" }
        if model.discoveryPresentation.settledCoverageNeedsAttention { return "Some changed folders still need checking" }
        if stats.complete { return stats.errors > 0 ? "Scan finished with inaccessible items" : "Scan complete" }
        if stats.entries > 0 { return "Partial scan coverage" }
        return "Ready for a look round"
    }
    private var attention: Bool {
        stats.cancelled || stats.errors > 0 || model.discoveryPresentation.settledCoverageNeedsAttention
    }
    private var statusHelp: String {
        if model.discoveryPresentation.isRequestPending { return headline }
        let roots = model.snapshot.roots.count
        let counts = "\(roots) \(roots == 1 ? "folder" : "folders") · \(stats.excludedArtifacts.formatted()) artifacts skipped · \(stats.errors.formatted()) inaccessible"
        if model.discoveryPresentation.settledCoverageNeedsAttention {
            let history = model.snapshot.foregroundScan == nil ? "" : "The counts are from the last scan. "
            return "\(headline)\n\(counts)\n\(history)Current coverage is incomplete; scan again to recheck coverage."
        }
        return stats.message.isEmpty ? "\(headline)\n\(counts)" : "\(headline)\n\(counts)\n\(stats.message)"
    }
    private var line: String {
        model.discoveryPresentation.statusLine(rootCount: model.snapshot.roots.count)
    }
    /// A user-requested scan shows Cancel immediately through the foreground
    /// presentation; background maintenance earns its Pause control only after
    /// the debounced activity signal settles.
    private var showsScanControl: Bool {
        !model.hasCleanupWork && (model.discoveryPresentation.isForeground || model.backgroundActivityVisible)
    }

    var body: some View {
        HStack(spacing: 7) {
            if model.discoveryPresentation.isForeground && !model.hasCleanupWork {
                ProgressView().controlSize(.small).scaleEffect(0.6).frame(width: 13, height: 13)
            } else {
                Image(systemName: stats.complete && !attention ? "checkmark.circle" : "circle.dashed")
                    .font(TeaFont.caption)
                    .foregroundStyle(attention ? TeaTheme.rust : TeaTheme.inkSoft)
                    .frame(width: 13, height: 13)
            }
            Text(line)
                .font(TeaFont.caption).monospacedDigit().foregroundStyle(TeaTheme.inkSoft)
                .lineLimit(1).minimumScaleFactor(0.82)
            Spacer(minLength: 4)
            // Retain the original Cancel label's footprint for both actions,
            // including while idle, without changing the button's ink geometry.
            // Visibility follows the debounced activity signal, never the raw
            // scanning flag, so event-driven background workers cannot strobe it.
            Button { model.cancel() } label: {
                Text("Cancel").hidden()
                    .overlay { Text(model.discoveryPresentation.scanControlTitle) }
            }
                .buttonStyle(InkButtonStyle(kind: .quiet, compact: true, seed: 291))
                .opacity(showsScanControl ? 1 : 0)
                .disabled(!showsScanControl)
                .allowsHitTesting(showsScanControl)
                .accessibilityHidden(!showsScanControl)
                .help(model.discoveryPresentation.scanControlHelp)
                .accessibilityLabel(model.discoveryPresentation.scanControlAccessibilityLabel)
                .accessibilityHint(model.discoveryPresentation.scanControlHelp)
                .animation(.easeInOut(duration: 0.22), value: showsScanControl)
        }
        .help(statusHelp)
        .accessibilityElement(children: .combine)
        .accessibilityLabel("\(headline). \(line)")
    }
}

private struct CandidateRow: View {
    @ObservedObject var model: AppModel
    let candidate: Candidate
    var seed: Int
    private var changing: Bool { model.busy || model.snapshot.cleaning }
    private var selectionAllowed: Bool { model.canEnqueueCleanup && candidate.canReviewCleanup }
    private var atSelectionLimit: Bool { !model.selection.contains(candidate.id) && model.selection.count >= 100 }
    private var isSelected: Bool { model.selection.contains(candidate.id) }
    private var metadataLabel: String {
        guard candidate.isPersonalFile else { return candidate.category }
        let fileType = URL(fileURLWithPath: candidate.path).pathExtension.uppercased()
        let type = fileType.isEmpty ? "Local file" : "\(fileType) file"
        guard candidate.modifiedNs > 0 else { return type }
        let date = Date(timeIntervalSince1970: Double(candidate.modifiedNs) / 1_000_000_000)
        return "\(type) · Modified \(date.formatted(date: .abbreviated, time: .omitted))"
    }
    private var selected: Binding<Bool> {
        Binding(get: { candidate.canReviewCleanup && model.selection.contains(candidate.id) }, set: { value in
            guard selectionAllowed, !value || !atSelectionLimit else { return }
            if value { model.selection.insert(candidate.id) } else { model.selection.remove(candidate.id) }
        })
    }

    var body: some View {
        let shape = WobblyRect(radius: TeaTheme.cardRadius, seed: seed)
        VStack(alignment: .leading, spacing: 6) {
            HStack(alignment: .top, spacing: 9) {
                Toggle("Select \(candidate.displayName)", isOn: selected)
                    .toggleStyle(InkCheckboxStyle(seed: seed &+ 3))
                    .padding(.top, 7)
                    .disabled(!selectionAllowed || atSelectionLimit)
                    .help(atSelectionLimit ? "Select up to 100 items at a time." : "Select this item for cleanup")
                    .accessibilityLabel("Select \(candidate.displayName). Path: \(candidate.path)")
                ArtifactIcon(candidate: candidate, size: 32, seed: seed &+ 5)
                VStack(alignment: .leading, spacing: 2) {
                    Text(candidate.displayName).font(TeaFont.bodySemibold).lineLimit(1).truncationMode(.middle).help(metadataLabel)
                    Text(candidate.category).font(TeaFont.captionMedium).foregroundStyle(TeaTheme.inkSoft).lineLimit(1)
                    Text(candidate.path).font(TeaFont.mono).foregroundStyle(TeaTheme.inkSoft)
                        .lineLimit(1).truncationMode(.head).help(candidate.path).textSelection(.enabled)
                }
                Spacer(minLength: 4)
                VStack(alignment: .trailing, spacing: 0) {
                    Text(space(candidate.allocatedBytes)).font(TeaFont.bodyNumber).monospacedDigit()
                    Text("estimated").font(TeaFont.caption).foregroundStyle(TeaTheme.inkSoft)
                }
                .fixedSize()
            }
            Text(candidate.explanation).font(TeaFont.caption).foregroundStyle(TeaTheme.inkSoft)
                .lineLimit(2).fixedSize(horizontal: false, vertical: true)
            HStack(alignment: .top, spacing: 5) {
                Image(systemName: candidate.canReviewCleanup ? "arrow.turn.down.right" : "lock").font(TeaFont.caption).padding(.top, 1)
                Text(candidate.cleanupBlockedReason ?? candidate.consequence)
                    .font(TeaFont.caption).lineLimit(2).fixedSize(horizontal: false, vertical: true)
            }
            .foregroundStyle(candidate.canReviewCleanup ? TeaTheme.inkSoft : TeaTheme.rust)
            InkDivider(seed: seed &+ 7)
            HStack(spacing: 6) {
                if permanentCleanupEligible(candidate) {
                    Text(potentialReward(candidate)).font(TeaFont.caption).foregroundStyle(TeaTheme.goldDeep)
                        .lineLimit(1).minimumScaleFactor(0.8)
                } else {
                    Text(candidate.canReviewCleanup ? "Trash only" : "Unavailable for cleanup")
                        .font(TeaFont.caption).foregroundStyle(TeaTheme.inkSoft).fixedSize()
                }
                Spacer(minLength: 4)
                Menu {
                    Button("Quick Look") { model.quickLook(path: candidate.path) }
                    Button("Reveal in Finder") { model.reveal(path: candidate.path) }
                    Divider()
                    Button("Keep this item") { model.keep(candidate) }.disabled(changing || model.scanActivityForUI)
                } label: {
                    Image(systemName: "ellipsis").font(TeaFont.bodySemibold).frame(width: 22, height: 16)
                }
                .menuStyle(.borderlessButton).menuIndicator(.hidden).fixedSize()
                .foregroundStyle(TeaTheme.inkSoft)
                .accessibilityLabel("Actions for \(candidate.displayName). Path: \(candidate.path)")
            }
        }
        .padding(.horizontal, 11).padding(.vertical, 9)
        .background { shape.fill(TeaTheme.card).shadow(color: TeaTheme.ink.opacity(0.10), radius: 1.5, y: 1.5) }
        .overlay(shape.stroke(isSelected ? TeaTheme.biro : TeaTheme.ink.opacity(0.8),
                              style: StrokeStyle(lineWidth: isSelected ? 1.9 : TeaTheme.inkLine, lineCap: .round, lineJoin: .round)))
        .contextMenu {
            Button("Quick Look") { model.quickLook(path: candidate.path) }
            Button("Reveal in Finder") { model.reveal(path: candidate.path) }
            Button("Keep this item") { model.keep(candidate) }.disabled(changing || model.scanActivityForUI)
        }
    }
}

private func permanentCleanupEligible(_ item: Candidate) -> Bool {
    item.canDeletePermanently
}

private func potentialReward(_ item: Candidate) -> String {
    let count = item.allocatedBytes / 100_000_000
    if count == 0 { return "Permanent: scraps towards your next chip" }
    return "Permanent: up to \(chipsPhrase(count)), if credited"
}

private struct FolderEmptyState: View {
    @ObservedObject var model: AppModel

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Spacer(minLength: 4)
            MagnifierDoodle(size: 60).frame(maxWidth: .infinity)
            Text("Have a proper look round.").font(TeaFont.title).frame(maxWidth: .infinity, alignment: .center)
            ScanEverywhereCTA(model: model, seed: 301)
            Spacer(minLength: 14)
        }
        .frame(maxWidth: .infinity)
        .padding(.horizontal, TeaTheme.panelPadding)
    }
}

private enum EmptyDoodle { case mug, magnifier }

private struct EmptyState: View {
    let doodle: EmptyDoodle
    let title: String
    let detail: String

    var body: some View {
        VStack(spacing: 7) {
            switch doodle {
            case .mug: MugDoodle(size: 56)
            case .magnifier: MagnifierDoodle(size: 56)
            }
            Text(title).font(TeaFont.title)
            Text(detail).font(TeaFont.caption).foregroundStyle(TeaTheme.inkSoft)
                .multilineTextAlignment(.center).lineSpacing(3)
        }
        .frame(maxWidth: .infinity)
        .padding(.horizontal, 12).padding(.vertical, 11)
    }
}

// MARK: - Review takeover

private struct ReviewTakeover: View {
    @ObservedObject var model: AppModel
    @State private var dontAskAgain = false

    private var trashEligible: Bool { !model.reviewItems.isEmpty && model.reviewItems.allSatisfy { $0.blockedReason == nil } }
    private var permanentEligible: Bool { !model.reviewItems.isEmpty && model.reviewItems.allSatisfy(permanentCleanupEligible) }
    private var estimatedBytes: UInt64 { model.reviewItems.reduce(0) { $0 &+ $1.allocatedBytes } }
    private var reviewIsCurrent: Bool {
        let current = Set(model.displayedCandidates)
        let authorized = Set(model.snapshot.roots.map(\.id))
        return !model.reviewItems.isEmpty && model.reviewItems.allSatisfy { current.contains($0) && authorized.contains($0.rootId) }
    }
    private var maySubmit: Bool { model.canEnqueueCleanup && reviewIsCurrent }

    var body: some View {
        VStack(spacing: 0) {
            header
            ScrollView {
                VStack(alignment: .leading, spacing: 8) {
                    ForEach(Array(model.reviewItems.enumerated()), id: \.element.id) { index, item in
                        ReviewItemCard(item: item, seed: 321 + index * 6)
                    }
                    if !reviewIsCurrent { staleWarning }
                    if model.discoveryPresentation.isForeground {
                        Text(model.hasCleanupWork
                             ? "Discovery resumes when the queued cleanup finishes."
                             : "Discovery continues while you review. It pauses for cleanup, then resumes.")
                            .font(TeaFont.caption).foregroundStyle(TeaTheme.biro)
                            .fixedSize(horizontal: false, vertical: true).lineSpacing(2)
                    }
                    HStack(alignment: .top, spacing: 7) {
                        Image(systemName: "info.circle").font(TeaFont.caption).padding(.top, 1)
                        Text("Every item is checked again before removal. Changed items need a fresh review. Freed space and chips may be lower than estimates.")
                            .font(TeaFont.caption).fixedSize(horizontal: false, vertical: true).lineSpacing(2)
                    }
                    .foregroundStyle(TeaTheme.inkSoft).padding(.top, 1)
                }
                .padding(.horizontal, TeaTheme.panelPadding).padding(.top, 2).padding(.bottom, 12)
            }
            .scrollIndicators(.hidden)
            footer
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(TeaTheme.paper)
        .onChange(of: model.reviewItems) { _, _ in dontAskAgain = false }
        .onChange(of: model.displayedCandidates) { _, _ in if !reviewIsCurrent { dontAskAgain = false } }
    }

    private var header: some View {
        VStack(alignment: .leading, spacing: 7) {
            Button { model.showReview = false } label: {
                HStack(spacing: 4) {
                    Image(systemName: "chevron.left").font(TeaFont.caption)
                    Text("Back")
                }
            }
            .buttonStyle(InkButtonStyle(kind: .quiet, compact: true, seed: 311))
            .keyboardShortcut(.cancelAction)
            .accessibilityLabel("Back to findings")

            VStack(alignment: .leading, spacing: 3) {
                ScreenTitle(text: "One last look.", seed: 313)
                Text("\(model.reviewItems.count) \(model.reviewItems.count == 1 ? "item" : "items") · \(space(estimatedBytes)) estimated on disk")
                    .font(TeaFont.caption).monospacedDigit().foregroundStyle(TeaTheme.inkSoft)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal, TeaTheme.panelPadding)
        .padding(.top, 10).padding(.bottom, 9)
    }

    private var staleWarning: some View {
        InkCard(padding: 9, seed: 315, fill: TeaTheme.card, stroke: TeaTheme.rust) {
            VStack(alignment: .leading, spacing: 3) {
                Label("This review needs an update", systemImage: "exclamationmark.circle")
                    .font(TeaFont.bodySemibold)
                Text("Something changed while this was open. Go back and review the current finding.")
                    .font(TeaFont.caption).fixedSize(horizontal: false, vertical: true).lineSpacing(2)
            }
            .foregroundStyle(TeaTheme.rust)
        }
    }

    private var footer: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(alignment: .top, spacing: 8) {
                Image(systemName: permanentEligible ? "exclamationmark.circle" : "trash")
                    .font(TeaFont.body).padding(.top, 1)
                VStack(alignment: .leading, spacing: 2) {
                    Text(permanentEligible ? "Permanent cleanup" : "Move to Trash")
                        .font(TeaFont.bodySemibold)
                    Text(permanentEligible
                         ? "Deletes these developer files without Trash or restore. Freed space and chips may be zero."
                         : "Recoverable from Activity until Trash is emptied. Moving files there does not free space or earn chips.")
                        .font(TeaFont.caption).foregroundStyle(TeaTheme.inkSoft).fixedSize(horizontal: false, vertical: true)
                }
            }
            if permanentEligible {
                Toggle("Don't ask again", isOn: $dontAskAgain)
                    .toggleStyle(InkCheckboxStyle(seed: 341, showsLabel: true))
                    .font(TeaFont.caption)
                    .disabled(!maySubmit)
                    .accessibilityHint("Turns off future deletion confirmations only when you confirm this deletion. Change it again in Settings.")
                Button("Delete permanently") {
                    guard maySubmit && permanentEligible else { return }
                    if dontAskAgain { model.confirmBeforeDeleting = false }
                    model.clean(permanently: true)
                }
                .buttonStyle(InkButtonStyle(kind: .destructive, fullWidth: true, seed: 333))
                .disabled(!maySubmit)
                .accessibilityHint(model.hasCleanupWork
                    ? "Queues the exact reviewed items for permanent deletion after the current cleanup. No Trash or restore."
                    : "Permanently deletes the exact reviewed items. No Trash or restore.")
                InkDivider(seed: 335)
                VStack(alignment: .leading, spacing: 5) {
                    Button("Move to Trash instead") { if maySubmit && trashEligible { model.clean(permanently: false) } }
                        .buttonStyle(InkButtonStyle(kind: .quiet, fullWidth: true, compact: true, seed: 337))
                        .disabled(!maySubmit || !trashEligible)
                        .accessibilityHint(model.hasCleanupWork
                            ? "Queues the reviewed items to move to Trash after the current cleanup. Earns no chips."
                            : "Moves the reviewed items to Trash. Earns no chips.")
                    Text("Recoverable from Activity until Trash is emptied.")
                        .font(TeaFont.caption).foregroundStyle(TeaTheme.inkSoft).fixedSize(horizontal: false, vertical: true)
                }
            } else {
                Button("Move to Trash") { if maySubmit && trashEligible { model.clean(permanently: false) } }
                    .buttonStyle(InkButtonStyle(kind: .primary, fullWidth: true, seed: 333))
                    .disabled(!maySubmit || !trashEligible)
                    .accessibilityHint(model.hasCleanupWork
                        ? "Queues the reviewed items to move to Trash after the current cleanup. Earns no chips."
                        : "Moves the reviewed items to Trash. Earns no chips.")
                Text("App caches, logs, Xcode build data and personal files go to Trash only. Permanent cleanup is limited to eligible developer artifacts.")
                    .font(TeaFont.caption).foregroundStyle(TeaTheme.inkSoft)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .padding(.horizontal, TeaTheme.panelPadding).padding(.top, 8).padding(.bottom, 10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(TeaTheme.paperDeep.opacity(0.7))
        .overlay(alignment: .top) {
            WobblyLine(amplitude: 0.8, seed: 339)
                .stroke(TeaTheme.ink.opacity(0.35), style: StrokeStyle(lineWidth: 1.2, lineCap: .round))
                .frame(height: 3)
        }
    }
}

private struct ReviewItemCard: View {
    let item: Candidate
    var seed: Int

    var body: some View {
        InkCard(padding: 9, seed: seed) {
            VStack(alignment: .leading, spacing: 7) {
                HStack(alignment: .top, spacing: 10) {
                    ArtifactIcon(candidate: item, size: 30, seed: seed &+ 5)
                    VStack(alignment: .leading, spacing: 2) {
                        Text(item.displayName).font(TeaFont.bodySemibold).lineLimit(1).truncationMode(.middle)
                        Text(item.category).font(TeaFont.captionMedium).foregroundStyle(TeaTheme.inkSoft).lineLimit(1)
                        Text(item.path).font(TeaFont.mono).foregroundStyle(TeaTheme.inkSoft)
                            .textSelection(.enabled).fixedSize(horizontal: false, vertical: true)
                    }
                    Spacer(minLength: 4)
                    VStack(alignment: .trailing, spacing: 0) {
                        Text(space(item.allocatedBytes)).font(TeaFont.bodyNumber).monospacedDigit()
                        Text("estimated").font(TeaFont.caption).foregroundStyle(TeaTheme.inkSoft)
                    }
                    .fixedSize()
                }
                Text(item.explanation).font(TeaFont.caption).foregroundStyle(TeaTheme.inkSoft)
                    .fixedSize(horizontal: false, vertical: true)
                if let reason = item.cleanupBlockedReason {
                    Label(reason, systemImage: "lock")
                        .font(TeaFont.caption).foregroundStyle(TeaTheme.rust)
                        .fixedSize(horizontal: false, vertical: true)
                }
                HStack(alignment: .top, spacing: 5) {
                    Image(systemName: "arrow.turn.down.right").font(TeaFont.caption).padding(.top, 1)
                    Text(item.consequence).font(TeaFont.caption)
                        .fixedSize(horizontal: false, vertical: true).lineSpacing(2)
                }
                .padding(8).frame(maxWidth: .infinity, alignment: .leading)
                .background(WobblyRect(radius: 8, amplitude: 0.7, seed: seed &+ 9, step: 12).fill(TeaTheme.paperDeep.opacity(0.7)))
                .overlay(WobblyRect(radius: 8, amplitude: 0.7, seed: seed &+ 9, step: 12).stroke(TeaTheme.ink.opacity(0.28), lineWidth: 1.1))
                HStack(spacing: 6) {
                    Text("\(space(item.logicalBytes)) file size · \(item.fileCount.formatted()) files")
                        .lineLimit(1).minimumScaleFactor(0.85)
                    Spacer(minLength: 4)
                    if permanentCleanupEligible(item) {
                        Text(potentialReward(item)).foregroundStyle(TeaTheme.goldDeep)
                            .lineLimit(1).minimumScaleFactor(0.8)
                    }
                }
                .font(TeaFont.caption).foregroundStyle(TeaTheme.inkSoft)
            }
        }
    }
}

// MARK: - Activity

/// The order book: one slim line per receipt, built to stay light however long
/// the ledger grows. Rows are lazy, fixed-shape and Equatable, identified and
/// seeded by receipt id (never list position), so new receipts do not redraw
/// old ones. Details open in place; older pages stream in on request.
private struct ActivityPage: View {
    @ObservedObject var model: AppModel
    @Environment(\.accessibilityReduceMotion) private var systemReduceMotion
    /// Rendered rows are bounded; the ledger behind them can be any size.
    @State private var visibleLimit = 150
    private var reduced: Bool { model.reduceMotion || systemReduceMotion }
    private var actionsDisabled: Bool { model.busy || model.snapshot.cleaning || model.scanActivityForUI }

    var body: some View {
        let receipts = model.displayedReceipts
        let shown = receipts.count > visibleLimit ? Array(receipts.prefix(visibleLimit)) : receipts
        let total = max(model.historyTotal ?? 0, UInt64(receipts.count))
        VStack(spacing: 0) {
            HStack(spacing: 6) {
                ScreenTitle(text: "Activity", seed: 351)
                Spacer(minLength: 0)
                Text("\(total.formatted()) \(total == 1 ? "order" : "orders")")
                    .font(TeaFont.caption).monospacedDigit().foregroundStyle(TeaTheme.inkSoft)
            }
            .padding(.horizontal, TeaTheme.panelPadding)
            .padding(.top, 8).padding(.bottom, 6)

            if receipts.isEmpty {
                Spacer(minLength: 0)
                EmptyState(doodle: .mug, title: "No orders in yet.",
                           detail: "Cleanup receipts live here: what happened, what was credited, and a way back for Trash items.")
                Spacer(minLength: 34)
            } else {
                ScrollView {
                    LazyVStack(spacing: 0) {
                        ForEach(shown) { receipt in
                            ReceiptRow(model: model, receipt: receipt,
                                       expanded: model.expandedReceipt == receipt.id,
                                       actionsDisabled: actionsDisabled)
                                .equatable()
                        }
                        if receipts.count > visibleLimit || model.hasOlderReceipts {
                            olderControls(loaded: receipts.count)
                        }
                    }
                    .padding(.horizontal, TeaTheme.panelPadding)
                    .padding(.top, 2).padding(.bottom, 16)
                }
                .scrollIndicators(.hidden)
                .animation(reduced ? nil : .spring(response: 0.26, dampingFraction: 0.9), value: model.expandedReceipt)
            }
        }
    }

    private func olderControls(loaded: Int) -> some View {
        VStack(spacing: 6) {
            Button(model.loadingOlderReceipts ? "Fetching older orders…" : "Show older orders") {
                visibleLimit += 250
                if loaded < visibleLimit { model.loadOlderReceipts() }
            }
            .buttonStyle(InkButtonStyle(kind: .quiet, fullWidth: true, compact: true, seed: 355))
            .disabled(model.loadingOlderReceipts)
            .accessibilityHint("Loads earlier receipts from the local ledger.")
        }
        .padding(.top, 10)
    }
}

/// One line of the order book. The model reference is an action target; the
/// row renders only from the receipt, its expansion, and the shared disabled
/// flag, so scanning and wallet churn never invalidate settled rows.
private struct ReceiptRow: View, Equatable {
    let model: AppModel
    let receipt: Receipt
    let expanded: Bool
    let actionsDisabled: Bool

    nonisolated static func == (lhs: Self, rhs: Self) -> Bool {
        lhs.model === rhs.model && lhs.receipt == rhs.receipt
            && lhs.expanded == rhs.expanded && lhs.actionsDisabled == rhs.actionsDisabled
    }

    /// Ink jitter follows the receipt's identity, not its list position, so an
    /// arriving receipt cannot re-scribble every row beneath it.
    private var seed: Int {
        var hash: UInt64 = 1_469_598_103_934_665_603
        for byte in receipt.id.utf8 { hash = (hash ^ UInt64(byte)) &* 1_099_511_628_211 }
        return Int(truncatingIfNeeded: hash & 0x7FFF_FFFF)
    }
    private var isTrash: Bool { receipt.operation == "trash" }
    private var needsAttention: Bool { ["failed", "partial", "skipped", "interrupted", "cancelled"].contains(receipt.outcome) }
    private var date: Date {
        Date(timeIntervalSince1970: Double(receipt.createdAt) / (receipt.createdAt > 1_000_000_000_000 ? 1000 : 1))
    }
    private var operationTitle: String {
        switch receipt.operation {
        case "trash":
            if receipt.outcome == "restored" { return "Restored from Trash" }
            return receipt.outcome == "trashed" || receipt.outcome == "success" ? "Moved to Trash" : "Move to Trash"
        case "permanent": return "Permanent cleanup"
        case "restore": return "Restore from Trash"
        default: return receipt.operation.capitalized
        }
    }
    private var symbol: String {
        if receipt.outcome == "restored" || receipt.operation == "restore" { return "arrow.uturn.backward" }
        return isTrash ? "trash" : "doc.text.magnifyingglass"
    }
    private var metrics: String {
        "Reported \(space(receipt.reportedBytes)) · Observed \(isTrash ? "not measured" : space(receipt.observedBytes)) · Credited \(space(receipt.creditedBytes)) · \(chipsPhrase(receipt.coins))"
    }
    private var accessibleSummary: String {
        var parts = "\(receipt.title). \(operationTitle), \(receipt.outcome.replacingOccurrences(of: "_", with: " "))"
        parts += ", \(space(receipt.reportedBytes))"
        if receipt.coins > 0 { parts += ", earned \(chipsPhrase(receipt.coins))" }
        return parts
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Button {
                model.expandedReceipt = expanded ? nil : receipt.id
            } label: {
                HStack(spacing: 9) {
                    Image(systemName: symbol)
                        .font(TeaFont.body)
                        .foregroundStyle(needsAttention ? TeaTheme.rust : TeaTheme.ink)
                        .frame(width: 24, height: 24)
                        .background(WobblyRect(radius: 7, amplitude: 0.7, seed: seed &+ 3, step: 7)
                            .fill(receipt.coins > 0 ? TeaTheme.gold.opacity(0.20) : TeaTheme.paperDeep.opacity(0.7)))
                        .overlay(WobblyRect(radius: 7, amplitude: 0.7, seed: seed &+ 3, step: 7)
                            .stroke(TeaTheme.ink.opacity(0.35), lineWidth: 1))
                    VStack(alignment: .leading, spacing: 1) {
                        Text(receipt.title).font(TeaFont.bodyMedium).lineLimit(1)
                        HStack(spacing: 4) {
                            if needsAttention {
                                Circle().fill(TeaTheme.rust).frame(width: 5, height: 5)
                            }
                            Text("\(operationTitle) · \(date.formatted(.dateTime.day().month(.abbreviated)))")
                                .font(TeaFont.caption)
                                .foregroundStyle(needsAttention ? TeaTheme.rust : TeaTheme.inkSoft)
                                .lineLimit(1)
                        }
                    }
                    Spacer(minLength: 6)
                    VStack(alignment: .trailing, spacing: 1) {
                        Text(space(receipt.reportedBytes)).font(TeaFont.bodyNumber).monospacedDigit()
                        if receipt.coins > 0 {
                            Text("+\(receipt.coins.formatted()) \(receipt.coins == 1 ? "chip" : "chips")")
                                .font(TeaFont.captionMedium).monospacedDigit().foregroundStyle(TeaTheme.goldDeep)
                        }
                    }
                    .fixedSize()
                    Image(systemName: "chevron.down")
                        .font(TeaFont.caption).foregroundStyle(TeaTheme.inkSoft)
                        .rotationEffect(.degrees(expanded ? 180 : 0))
                }
                .padding(.vertical, 7)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel(accessibleSummary)
            .accessibilityHint(expanded ? "Closes the receipt details." : "Opens the receipt details.")
            .accessibilityAddTraits(expanded ? [.isSelected] : [])

            if expanded { details }
            InkDivider(seed: seed &+ 9)
        }
    }

    private var details: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(receipt.path).font(TeaFont.mono).foregroundStyle(TeaTheme.inkSoft)
                .textSelection(.enabled).fixedSize(horizontal: false, vertical: true)
            Text(receipt.detail).font(TeaFont.caption).foregroundStyle(TeaTheme.inkSoft)
                .lineSpacing(2).fixedSize(horizontal: false, vertical: true)
            Text(metrics)
                .font(TeaFont.captionMedium).monospacedDigit().foregroundStyle(TeaTheme.ink)
                .fixedSize(horizontal: false, vertical: true).lineSpacing(2)
                .help("Reported size before the operation, observed free-space increase, space credited by the reward policy, and what it earned.")
            HStack(spacing: 8) {
                Text(receipt.outcome.replacingOccurrences(of: "_", with: " ").capitalized)
                    .font(TeaFont.captionMedium)
                    .foregroundStyle(needsAttention ? TeaTheme.rust : TeaTheme.ink)
                    .padding(.horizontal, 7).padding(.vertical, 3)
                    .background(WobblyPill(seed: seed &+ 11).fill(needsAttention ? TeaTheme.rust.opacity(0.10) : TeaTheme.gold.opacity(0.20)))
                    .overlay(WobblyPill(seed: seed &+ 11).stroke((needsAttention ? TeaTheme.rust : TeaTheme.ink).opacity(0.5), lineWidth: 1))
                    .fixedSize()
                Spacer(minLength: 0)
                if receipt.outcome == "restored" {
                    Button("Show in Finder") { model.reveal(path: receipt.path) }
                        .buttonStyle(InkButtonStyle(kind: .quiet, compact: true, seed: seed &+ 15))
                } else if isTrash, let trashPath = receipt.trashPath {
                    Button("Show in Finder") { model.reveal(path: trashPath) }
                        .buttonStyle(InkButtonStyle(kind: .quiet, compact: true, seed: seed &+ 15))
                }
                if receipt.canRestore && isTrash {
                    Button("Restore") { model.restore(receipt) }
                        .buttonStyle(InkButtonStyle(kind: .quiet, compact: true, seed: seed &+ 17))
                        .disabled(actionsDisabled)
                        .accessibilityHint("Restores only if the original location is empty and identities still match. Existing files are never overwritten.")
                }
            }
        }
        .padding(.leading, 33).padding(.bottom, 9)
        .transition(.opacity)
    }
}

// MARK: - Settings

private struct SettingsPage: View {
    @ObservedObject var model: AppModel
    @ObservedObject var updates: UpdateController
    @Environment(\.accessibilityReduceMotion) private var systemReduceMotion
    private var changing: Bool { model.busy || model.snapshot.cleaning || model.scanActivityForUI }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 11) {
                ScreenTitle(text: "Settings", seed: 371)

                SettingsSection(title: "App updates", seed: 372) {
                    HStack(alignment: .firstTextBaseline) {
                        Text("chippytea \(Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "development")")
                            .font(TeaFont.bodySemibold)
                        Spacer(minLength: 8)
                        Button(updates.availableVersion == nil ? "Check for updates" : "Download update…") {
                            updates.checkForUpdates()
                        }
                        .buttonStyle(InkButtonStyle(kind: .quiet, compact: true, seed: 374))
                        .disabled(!updates.canCheckForUpdates)
                    }
                    if let message = updates.statusMessage {
                        Text(message).font(TeaFont.caption).foregroundStyle(TeaTheme.inkSoft)
                            .fixedSize(horizontal: false, vertical: true)
                    } else if let checked = updates.lastCheckDate {
                        Text("Last checked \(checked.formatted(date: .abbreviated, time: .shortened))")
                            .font(TeaFont.caption).foregroundStyle(TeaTheme.inkSoft)
                    }
                    if updates.isEnabled {
                        InkDivider(seed: 378)
                        settingToggle(title: "Check automatically",
                                      detail: "A quiet reminder here when a new version is ready.",
                                      symbol: "arrow.triangle.2.circlepath",
                                      binding: $updates.automaticallyChecksForUpdates)
                        settingToggle(title: "Download updates in the background",
                                      detail: "Install when you quit, or choose when to restart. Cleanup finishes first.",
                                      symbol: "arrow.down.circle",
                                      binding: $updates.automaticallyDownloadsUpdates)
                            .disabled(!updates.automaticallyChecksForUpdates)
                    } else {
                        Text("Updates are available in the installed app, not in test or preview builds.")
                            .font(TeaFont.caption).foregroundStyle(TeaTheme.inkSoft)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }

                SettingsSection(title: "Make yourself at home", seed: 373) {
                    settingToggle(title: "Collection sounds",
                                  detail: "A little chime when your chips land, like the bell over the shop door.",
                                  symbol: "speaker.wave.2", binding: $model.soundEnabled)
                    InkDivider(seed: 375)
                    settingToggle(title: "Reduce Motion",
                                  detail: systemReduceMotion ? "Your Mac’s Reduce Motion preference is active." : "Keep the ink still: no boiling lines, flying chips or bounce.",
                                  symbol: "figure.stand",
                                  binding: Binding(get: { model.reduceMotion || systemReduceMotion }, set: { model.reduceMotion = $0 }))
                        .disabled(systemReduceMotion)
                    InkDivider(seed: 376)
                    settingToggle(title: "Confirm before deleting",
                                  detail: "Ask once before permanently deleting developer files.",
                                  symbol: "trash", binding: $model.confirmBeforeDeleting)
                }

                SettingsSection(title: "Folders you’ve invited in", seed: 377) {
                    ForEach(model.snapshot.roots) { root in
                        HStack(spacing: 10) {
                            Image(systemName: "folder").font(TeaFont.body).frame(width: 18)
                            VStack(alignment: .leading, spacing: 1) {
                                Text(root.name).font(TeaFont.bodyMedium).lineLimit(1)
                                Text(root.path).font(TeaFont.mono).foregroundStyle(TeaTheme.inkSoft)
                                    .lineLimit(1).truncationMode(.middle).help(root.path)
                            }
                            Spacer(minLength: 0)
                            Button { model.forgetRoot(root) } label: { Image(systemName: "minus.circle").font(TeaFont.body) }
                                .buttonStyle(.plain).foregroundStyle(TeaTheme.inkSoft)
                                .accessibilityLabel("Stop scanning \(root.name)")
                                .help("Remove folder authorisation; files stay where they are")
                                .disabled(changing)
                        }
                    }
                    if !model.snapshot.roots.isEmpty { InkDivider(seed: 379) }
                    if !model.homeAuthorized {
                        Button("Scan everywhere") { model.scanMyMac() }
                            .buttonStyle(InkButtonStyle(kind: .primary, fullWidth: true, compact: true, seed: 387))
                            .disabled(changing)
                            .accessibilityHint("Authorises your home folder in one step. Nothing is removed without your review.")
                    }
                    HStack(spacing: 8) {
                        Button("Full Disk Access…") { model.beginDiskAccessSetup() }
                            .buttonStyle(.plain)
                            .font(TeaFont.captionMedium).foregroundStyle(TeaTheme.biro)
                            .accessibilityHint("Guides you through scan access in macOS settings.")
                            .disabled(changing)
                        Spacer(minLength: 4)
                        Menu {
                            Button("Projects Folder…") { model.chooseFolder(kind: "projects") }
                            Button("Downloads Folder…") { model.chooseFolder(kind: "downloads") }
                            Button("Another Folder…") { model.chooseFolder(kind: "folder") }
                        } label: {
                            Text("Add folder…").font(TeaFont.captionMedium)
                        }
                        .menuStyle(.borderlessButton).menuIndicator(.hidden).fixedSize()
                        .foregroundStyle(TeaTheme.biro)
                        .disabled(changing)
                        .accessibilityLabel("Add a folder")
                    }
                    Text("A guided setup for a more complete scan. Optional.")
                        .font(TeaFont.caption).foregroundStyle(TeaTheme.inkSoft)
                        .fixedSize(horizontal: false, vertical: true)
                }

                SettingsSection(title: "Kept for a reason", seed: 381) {
                    if model.snapshot.keptPaths.isEmpty {
                        Text("Choose “Keep this item” on any finding to leave it out of future suggestions.")
                            .font(TeaFont.caption).foregroundStyle(TeaTheme.inkSoft)
                            .lineSpacing(2).fixedSize(horizontal: false, vertical: true)
                    } else {
                        ForEach(model.snapshot.keptPaths, id: \.self) { path in
                            HStack(spacing: 10) {
                                Image(systemName: "pin").font(TeaFont.caption).foregroundStyle(TeaTheme.inkSoft).frame(width: 14)
                                Text(path).font(TeaFont.mono)
                                    .lineLimit(1).truncationMode(.middle).help(path)
                                Spacer(minLength: 4)
                                Button("Include again") { model.unkeep(path: path) }
                                    .buttonStyle(InkButtonStyle(kind: .quiet, compact: true, seed: 383))
                                    .disabled(changing)
                            }
                        }
                    }
                }

                SettingsSection(title: "About", seed: 385) {
                    HStack(spacing: 8) {
                        BatteredFishLogo(height: 20)
                        Text("A proper chippy tea for your disk.").font(TeaFont.bodySemibold)
                    }
                    Text("One chip per 100 MB of space saved, on your Mac only. Anything smaller is scraps and carries forward. Trash earns no chips. Chips have no monetary value.")
                        .font(TeaFont.caption).foregroundStyle(TeaTheme.inkSoft)
                        .lineSpacing(2).fixedSize(horizontal: false, vertical: true)
                }
            }
            .padding(.horizontal, TeaTheme.panelPadding)
            .padding(.top, 8).padding(.bottom, 15)
        }
        .scrollIndicators(.hidden)
    }

    private func settingToggle(title: String, detail: String, symbol: String, binding: Binding<Bool>) -> some View {
        HStack(spacing: 10) {
            Image(systemName: symbol).font(TeaFont.body).frame(width: 18)
            VStack(alignment: .leading, spacing: 1) {
                Text(title).font(TeaFont.bodyMedium)
                Text(detail).font(TeaFont.caption).foregroundStyle(TeaTheme.inkSoft)
                    .fixedSize(horizontal: false, vertical: true)
            }
            Spacer(minLength: 8)
            Toggle(title, isOn: binding).labelsHidden().toggleStyle(.switch).controlSize(.mini)
        }
    }
}

/// Only updater subviews observe its state, so background checks do not
/// invalidate the scanner or the coin scene.
private struct UpdateBanner: View {
    @ObservedObject var updates: UpdateController

    var body: some View {
        if let version = updates.availableVersion {
            HStack(spacing: 8) {
                Image(systemName: "arrow.down.circle").foregroundStyle(TeaTheme.biro)
                Text("chippytea \(version) is ready").font(TeaFont.captionMedium)
                Spacer(minLength: 4)
                Button("Download update…") { updates.checkForUpdates() }
                    .buttonStyle(InkButtonStyle(kind: .quiet, compact: true, seed: 390))
                    .disabled(!updates.canCheckForUpdates)
            }
            .padding(.horizontal, TeaTheme.panelPadding)
            .padding(.vertical, 6)
            .accessibilityElement(children: .contain)
        }
    }
}

private struct UpdateInteractionGuard: ViewModifier {
    @ObservedObject var updates: UpdateController

    func body(content: Content) -> some View {
        // Block new work, but keep the masthead's existing cleanup Stop control
        // outside this subtree while a relaunch waits for that work to finish.
        content.disabled(updates.isInstalling)
    }
}

private struct SettingsSection<Content: View>: View {
    let title: String
    var seed: Int
    @ViewBuilder var content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            Text(title).font(TeaFont.captionSemibold).foregroundStyle(TeaTheme.inkSoft)
            InkCard(padding: 9, seed: seed) {
                VStack(alignment: .leading, spacing: 8) { content }
            }
        }
    }
}
