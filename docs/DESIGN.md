# chippytea design system

The design record for the tray panel. Implementation lives in `native/Chippytea/` (`CoinScene.swift` holds the tokens as `TeaTheme` and the hand-drawn rendering primitives); this document is the source of truth the code follows.

## Concept — the chippy tea

A "chippy tea" is the British fish-and-chips supper, and the panel is its paper: a page from someone's pocket notebook that doubles as the chip shop's wrap, taped up under the menu bar. Everything on it is drawn by hand in ink: a battered fish for a shop sign, wobbly outlines, scribble-shaded golden chips, hand-lettered numerals, small doodles in the margins. Nothing in the panel is a crisp geometric shape — if it has an edge, a hand drew it.

What follows from that image:

- **Fish and chips are the reward, and the home screen — the tab is simply Chippytea.** Tidy your disk, earn your tea: one chip per 100 MB of credited space, heaped in an open paper wrap, and every full thousand chips is a **battered fish** — the balance always reads as both, "0 fish, 900 chips". Fish are a display denomination of the same credited unit, never a separate ledger. Balance, paper, the next few cleanup opportunities; deeper tools are one tab away. The word "stash" does not appear in the interface. In copy the wrap is always **the paper** ("chips in the paper") — "wrap" is the drawn object only; as a noun in a sentence it reads like a sandwich, not a chippy.
- **The shop sign hangs top left.** The battered fish and the hand-lettered lowercase `chippytea` wordmark sit above every screen. The fish is also the monochrome menu-bar icon.
- **The drawing is alive.** Ink lines "boil" like frame-by-frame animation — but only where the eye is (the wrap, the tape, whatever you touch — hovering the shop sign counts). The page never vibrates at you.
- **One material.** Cream paper, charcoal ink, marigold gold, one ballpoint-blue accent. No panels-within-panels, no gradients, no glass.

## The panel

| Property | Value |
| --- | --- |
| Size | 380 pt wide, ~620 pt tall (clamped to screen) |
| Attachment | A marigold washi-tape strip straddling the top edge, centered under the status-item icon and tracking it when the panel is edge-clamped |
| Masthead | The shop sign, top left on every screen: battered fish (~26 pt) beside the hand-lettered `chippytea` wordmark; boils on hover |
| Entrance | Fade + 6 pt settle + 0.985→1 scale, ~180 ms ease-out; skipped under Reduce Motion |
| Dismissal | Click outside or Esc; system dialogs (folder picker, Quick Look) suspend auto-dismissal |
| Chrome | None. Not resizable, not movable. The page is the chrome. |

## Color

| Token | Value | Role |
| --- | --- | --- |
| `paper` | `#FAF5EA` | Panel ground — cream notebook stock, with an ink@5% dot grid |
| `paperDeep` | `#F1EADB` | Recessed and hover surfaces |
| `card` | `#FFFDF6` | Card slips, the chip wrap |
| `ink` | `#33302B` | Text and every drawn outline (warm pencil-charcoal) |
| `inkSoft` | `#7A736A` | Secondary text |
| `inkFaint` | ink @ 12% | Hairlines, the dot grid |
| `gold` / `goldDeep` | `#F2B63C` / `#C07F17` | Chips, batter, primary action, the tape |
| `biro` | `#3F5FA8` | Ballpoint-blue accent: active tab, selection, focus |
| `rust` | `#B4553D` | Destructive actions, warnings |

Rules: gold belongs to chips (and the battered fish), the primary action, and the tape — never decoration. Biro marks "where you are / what you chose." Rust only ever means "careful." Ink draws everything else.

## The hand-drawn language

Five primitives carry the whole aesthetic; nothing bypasses them:

1. **Ink strokes.** Every border, divider, and outline is a path perturbed by deterministic jitter (~1 pt amplitude, seeded) and stroked ~1.4 pt in ink. No crisp rectangles exist in the panel.
2. **Boiling lines.** Primitives take a boil phase; cycling the jitter seed at ~6 fps makes a line wriggle like hand animation. Boiling is rationed: the wrap and tape while the panel is open, a control you hover or press (the shop sign included), the collect burst. Everything else holds a static seed. Reduce Motion freezes all boiling. Nothing draws while the panel is closed.
3. **Hand-lettered numerals.** The balance is not a font: digits 0–9 are drawn stroke paths (~42 pt, fixed advance) with gold hatch fill and ink outlines, and the denominations — *fish* and *chips* — are written after their numbers in the same lettering, small and in plain ink, sitting on the digits' baseline. A digit that changes boils for a few frames. Only the balance earns this; body text stays a system face for legibility. The wordmark letters (`chippytea`, lowercase centre-line strokes on the same 92-unit box) are drawn the same way, stroked in plain ink.
4. **The chip.** A wobbly golden stick: gold fill, two fried `goldDeep` streaks, a crisped tip, ink outline, one catch of white light. The same chip at every size — heaped in the wrap, standing in threes as the chip-bundle doodle, tumbling in the collect burst. Its ghost (a dashed pale outline) marks the first chip yet to come.
5. **The battered fish.** The shop sign: one closed ink outline over scribble-hatched gold batter, an eye, a gill, tail creases, a wisp of steam. Full colour in the masthead and About; monochrome ink template in the menu bar. From the thousandth chip, the same fish (steam off) rides on top of the heap in the wrap, one per thousand, at most two drawn.

Component grammar built on those: buttons are wobbly-outlined boxes (primary = gold scribble-hatch fill; quiet = outline only; destructive = rust; pressing re-seeds the jitter, as if redrawn); checkboxes draw their checkmark as an ink stroke; filter chips are outlined pills; the active tab gets a hand-scribbled circle around its icon; screen titles are hand-underlined with one wobbly gold stroke; progress toward the next chip is a faint ruled line with a gold scribble drawn over the part already earned; the Chips tab icon is the chip bundle itself — ink outline at rest, golden when you are there. The (i) beside the numbers line is a real button: it opens a drawn slip, "How chips work", closed with one tap.

## Type

System fonts, rounded design, ink color: title 16 semibold, section heading 13.5 semibold, body 12, caption 10. No uppercase label typography anywhere — labels are sentence-case captions, merged into their value lines where possible ("Credited 1.2 GB"). The only display face is the hand-lettered numerals. Paths render in monospaced caption size, middle-truncated with a tooltip; monospace is reserved for paths.

## Space and shape

4 pt grid. Panel padding 20 horizontally, card padding 10; vertical rhythm is deliberately tight — a section header sits 12 pt from what precedes it, never 16. Corner radii are nominal (cards ~12, controls ~9) — the wobble makes them read as drawn, not rounded. Decorative elements (tape, doodles, title underlines) may rotate up to ~3°; text blocks never rotate.

## Motion

One orchestrated moment: collect — and it fires itself. A cleanup that finishes with chips earned while the panel is open switches to the Chips screen and plays the whole moment unprompted: chips tumble out from under the tape, trailed by short ink motion lines, into the wrap; the count rolls up in hand-lettered digits with a scrawled "+N" tally; the wrap squashes; a short rising chime plays. A 0-chip operation (Trash) gets a quiet fading ink note instead — never sound, never celebration. Everything else is functional: 150 ms tab crossfade, 180 ms entrance, checkmark draw-on ~150 ms. All motion honors the in-app toggle and system Reduce Motion; a completed credit is durable if its animation is interrupted.

## Doodles

Empty states get exactly one small static ink doodle: a steaming tea mug (activity), a magnifying glass (find space). Doodles never appear next to real data — with one earned exception: the **storage mug** on home is a gauge, not a doodle. The startup disk is drawn as the shop's mug of tea whose tea level is the free space, poured in with a short spring each time the panel opens and steaming while it is visible; the honest numbers ("412 GB free on this Mac / of 994 GB") sit beside it, with one flavour phrase ("plenty of room in the pot" → "nearly full — time for a tidy"). Reduce Motion serves the mug already settled.

## Scan setup

Full Disk Access is the one place the app has to explain macOS, so it gets three pages you click through instead of one page of instructions: **Permission**, **Drag it in**, **Switch it on** — numbered rings joined by a ruled line, the current one scribbled in biro, finished ones ticked. Every page is the same shape: one sketch, one headline, one sentence, one gold button, and "Choose a folder instead" underneath on every page. The sketches are the only looping animation outside the collect moment: a drawn System Settings window in which the app's card is lifted, carried along an arc and dropped into the list; then a drawn switch that flips on and a "Quit & Reopen" slip that gets clicked. They are drawn with the same ink, run at ~30 fps only while their page is on screen, and boil only on the thing that moves. Reduce Motion gets each instruction as one still: the card, the empty row and a dashed arrow between them; the switch already on with the slip answered. The real drag source sits under its sketch and looks exactly like the card in it, dashed like the ghost chip because it has not been placed yet; hovering it shows an open hand. After macOS quits and reopens the app, setup comes back on the last page.

## Voice

Gentle, small-scale, honest — and unmistakably British. "A bit more for the paper." — never "You saved 2 GB!!" Copy uses British spellings (-ise, towards) and chip-shop vocabulary where a flavour line is earned: chips come **in the paper**, the first chip is *still in the fryer*, an idle moment *sticks the kettle on*, Activity holds *orders*, and chips are *your tea*. Flavour lives in titles, empty states and asides; operational and consequence copy stays plain. macOS feature names (Trash, Finder, Full Disk Access) keep the system's own spelling. The reward system stays explainable in one breath: one chip per 100 MB of space conservatively credited; a thousand chips is a battered fish, the full supper; anything smaller is **scraps** — the chippy word for the leftover batter bits — carried forward, never lost. The honesty rules are design constraints, not legal copy: Trash labels are plain — **Move to Trash**, never a bolted-on chip figure — and the fact that Trash earns nothing is said once, in the consequence copy beside the action (the review footer, "How chips work", About), not on every label; sizes are always "estimated"; permanent rewards are always conditional ("up to N chips, if credited"); consequences stay visible in review; chips have no monetary value and never leave the Mac. Compression is allowed, weakening is not. (The engine and FFI keep their original `coins` field names; chips are the interface's presentation of the same unit.)

## UX principles

1. **Ninety percent of visits never leave home.** Balance, wrap, top three suggestions. Getting started is one click: "Scan my Mac" authorizes the home folder with no picker; specific folders are the secondary path.
2. **Two taps to clean, never one.** A suggested item expands inline with its size and consequence; the second, explicit tap performs it through the same validation as a full review. Multi-item selections get the full review takeover. Permanent deletion always requires the full review plus its own confirmation.
3. **The payoff is automatic.** Earned chips collect themselves the moment the cleanup finishes on screen; the user never hunts for a Collect button. Nothing celebrates a 0-chip operation.
4. **Power tools stay small.** Filters, sort and search exist on Find space but occupy two compact rows.
5. **Activity is an order book, not a stack of cards.** One slim line per receipt — icon, title, operation and date, size, and any gold "+N chips" — with details (path, honest metrics, outcome, Restore/Finder) opening in place as an accordion. Rows are lazy, fixed-shape and identity-seeded so the list stays light at any length; older orders page in from the ledger on request.
6. **Controls never flap.** Transient engine states (the scanning flag flips with every background worker) are debounced before they may show or hide a control: the Pause control appears only after background activity persists (~0.4 s) and stays through short gaps (~1.2 s). A user-requested scan shows Cancel immediately.
7. **The panel never traps.** Click-out always dismisses; nothing modal survives except an in-flight cleanup banner.
8. **One caption per section.** If a control already says it, no caption repeats it.
