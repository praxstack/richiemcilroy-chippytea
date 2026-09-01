<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/assets/chippytea-dark.svg">
    <source media="(prefers-color-scheme: light)" srcset="docs/assets/chippytea-light.svg">
    <img src="docs/assets/chippytea-light.svg" alt="Chippytea" width="310">
  </picture>
</p>

<h3 align="center">Room on your Mac, chips in the paper.</h3>

<p align="center">
  A native Mac app for finding old build files, making space, and earning your tea.<br>
  SwiftUI + Rust · macOS 14+ · Free and open source
</p>

<p align="center">
  <a href="#build-and-run">Build it</a> ·
  <a href="#how-it-works">How it works</a> ·
  <a href="CONTRIBUTING.md">Contribute</a> ·
  <a href="SECURITY.md">Security</a>
</p>

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/assets/readme-dark.svg">
  <source media="(prefers-color-scheme: light)" srcset="docs/assets/readme-light.svg">
  <img src="docs/assets/readme-light.svg" alt="Illustrated example: review an old node_modules folder, then turn 1.2 GB of credited space into 12 golden chips in a paper tray." width="1000">
</picture>

<p align="center"><sub>An illustrated cleanup, not a live scan. Animation respects reduced motion.</sub></p>

Chippytea lives in your menu bar. It looks for stale developer artifacts and old downloads, tells you what removing them means, and leaves the decision to you. No account, no telemetry, no automatic deletion.

## How it works

1. **Find a little room.** Scan folders you choose, or use the guided home-folder scan. Old `target` and `node_modules` folders are good places to start.
2. **Check before you clean.** Review the files, estimated size, and consequences. Tracked files, active projects, cloud-managed items, and uncertain candidates stay out.
3. **Earn your tea.** Eligible permanent cleanup earns one chip per **100 MB of conservatively credited space**. A thousand chips is a battered fish. Smaller amounts carry forward as scraps.

Moving something to **Trash earns no chips**. Downloads are Trash-only. Chips stay on your Mac and have no monetary value; estimated rewards never go straight into the wallet.

This is early software that can permanently delete files. Start with a disposable test folder and keep backups. Full Disk Access is optional. The [recommendation policy](docs/SUGGESTIONS.md) explains what qualifies, what stays out, and where the checks have limits.

## Build and run

You'll need an **Apple Silicon Mac**, **macOS 14 or later**, **Xcode with its command-line tools**, **Python 3**, and a [Rust toolchain](https://rustup.rs/).

```sh
git clone https://github.com/richiemcilroy/chippytea.git
cd chippytea
./scripts/build.sh
open build/Chippytea.app
```

Local builds are ad-hoc signed by default, not notarized releases. For toolchain versions, a safe test fixture, stable signing, and all test commands, see the [development guide](docs/development.md). Quit the running app before rebuilding.

```sh
env -u CARGO_TARGET_DIR -u CARGO_BUILD_TARGET_DIR cargo test --locked
cargo clippy --all-targets --locked -- -D warnings
```

Working on the landing page? See [site/README.md](site/README.md).

## Under the paper

| Part | What's inside |
| --- | --- |
| [Rust engine](core/src) | Bounded discovery, SQLite index, safety checks, cleanup, and a durable reward ledger |
| [Native app](native/Chippytea) | SwiftUI interface, menu-bar panel, Finder and Trash integration, hand-drawn fish and chips |
| [Landing page](site) | The same drawing paths and chip-shop style, built with Next.js |
| [Tests and benchmarks](scripts) | Disposable fixtures, native integration checks, and reproducible performance tools |

[Architecture](docs/ARCHITECTURE.md) · [Design](docs/DESIGN.md) · [Disk access](docs/DISK-ACCESS.md) · [Performance](docs/PERFORMANCE.md)

## Pull up a chair

Bug reports, careful safety tests, clearer docs, and small fixes are welcome. Read [CONTRIBUTING.md](CONTRIBUTING.md) before changing cleanup behaviour. Please keep personal paths, logs, and credentials out of public issues. Security problems belong in a [private report](https://github.com/richiemcilroy/chippytea/security/advisories/new).

[MIT licensed](LICENSE). Built with lessons from [Kondo, dua, and dust](docs/REFERENCE-STUDY.md).
