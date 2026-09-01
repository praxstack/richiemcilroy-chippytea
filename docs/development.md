# Development

The app is SwiftUI and AppKit over a Rust library. The landing page in `site/` is a separate Next.js app. Neither needs API keys for local development.

## Requirements

- macOS 14 or later. The native measurements currently documented in this repo are from Apple Silicon; they do not establish Intel compatibility.
- Xcode command-line tools with a Swift 6 toolchain.
- A current stable Rust toolchain, including `rustfmt` and `clippy`.
- Python 3 for the native build and benchmark helpers. Release checks need Python 3.12 or later.
- At least 3 GiB of free space **plus** the space for builds and disposable test payloads.
- Bun 1.4.0 for the landing page.

## Build the app

From the repository root:

```sh
./scripts/build.sh
```

This builds the release Rust library, links the Swift executable and produces `build/Chippytea.app`. Quit any running copy before rebuilding. Closing the window does not quit the menu-bar app.

The script uses ad-hoc signing by default. For a stable local identity, set `CHIPPYTEA_SIGNING_IDENTITY` to a certificate already in your keychain, or put the identity on one line in `.swiftpm/chippytea/signing-identity`. That local file stays out of Git. A configured identity must sign successfully; the build will not silently fall back. These commands do not notarize a distribution release.

Ad-hoc rebuilds can invalidate remembered disk permissions. Use one app bundle and read the [disk-access guide](DISK-ACCESS.md) before testing actual macOS consent. Full Disk Access is not needed for the disposable test suite.

## Checks

Run Rust formatting and lint checks:

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --locked -- -D warnings
```

Run the Rust tests, build and native checks:

```sh
./scripts/test.sh
```

For a Rust-only change, `cargo test --locked` runs the engine tests. Run the full native checks when changing the bridge, interface, permissions, cleanup or accounting.

`--self-test` exercises real native Trash, restoration, restart and permanent deletion using generated files. `--access-flow-test` checks isolated access setup and native interaction, including cleanup. Both retain local evidence and require space for their fixtures. They do not empty unrelated Trash or grant Full Disk Access. The access test simulates consent states; testing System Settings and Quit & Reopen still needs a manual check on the intended app build.

For updater or release changes, also run these checks after building the app:

```sh
python3 -B -m unittest discover -s scripts/release -p test_release.py -v
build/Chippytea.app/Contents/MacOS/Chippytea --update-self-test
```

The Python tests check version rules, archive paths, feed metadata and publication order using temporary files and mocked remote responses. The native update test checks cleanup gates, cancelled or failed installs, deferred relaunch and bundle configuration without starting a network update. Neither proves release signing, Apple notarization or an installed app's live update. See [Releasing Chippytea](RELEASING.md) for the release workflow and end-to-end checks.

## Manual testing without your real library

Use a fresh state directory so a development build cannot resume your saved scan grants:

```sh
chippytea_dev_state="$(mktemp -d "${TMPDIR:-/tmp}/chippytea-dev.XXXXXX")"
CHIPPYTEA_DATA_DIR="$chippytea_dev_state" \
  build/Chippytea.app/Contents/MacOS/Chippytea
```

Choose individual folders and select only a disposable fixture. Do not choose **Scan my Mac** for cleanup testing. Isolated state does not restrict which folders you can authorize.

To make a small, aged developer-artifact fixture, run this from another terminal at the repository root:

```sh
chippytea_fixture_parent="$(mktemp -d "${TMPDIR:-/tmp}/chippytea-fixture.XXXXXX")"
python3 scripts/make-fixture.py "$chippytea_fixture_parent/sample" \
  --files 100 --files-per-project 100 --bytes-per-file 1048576 \
  --age-days 9 --no-cases
```

Select the new `sample/baseline` folder. The generator refuses an existing destination and checks available space. Never remove that safeguard or substitute a real project. Inspect retained evidence before removing only the fixture directories you created.

## Landing page

```sh
cd site
bun install --frozen-lockfile
bun run dev
```

Use `bun run build` for the production build check. See [the site README](../site/README.md) for the shared illustration and animation code. Check both color schemes and reduced motion when changing the design.

## Further reading

- [Engine architecture](ARCHITECTURE.md): traversal, scheduling, mutation and persistence.
- [Recommendation policy](SUGGESTIONS.md): eligibility, exclusions and coverage limits.
- [Interface design](DESIGN.md): visual language and interaction.
- [Performance notes](PERFORMANCE.md): measurements and reproduction commands. Keep private raw output local.
- [Releasing Chippytea](RELEASING.md): signing, notarization, publishing and verifying in-app updates.

`MVP.md` is an earlier design proposal, not a list of shipped features. Use the source and current policy documents when describing behavior.
