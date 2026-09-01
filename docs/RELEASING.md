# Releasing Chippytea

Chippytea ships as a universal macOS 14+ app for Apple silicon and Intel. The
release workflow signs it with Developer ID, notarizes it with Apple, and
publishes a DMG for new installations and a ZIP for in-app updates.

Existing users can choose **Check for Updates…** in Chippytea. Sparkle handles
the download, verification, installation, and relaunch inside the app. A normal
update does not require manually downloading another DMG. macOS may still ask
for authorization if the installation location is not writable by the user.

Only a successful, published [GitHub release](https://github.com/richiemcilroy/chippytea/releases)
is a public build. Checking in this workflow does not itself publish a release.
Before the first successful release, the public update feed has no payload.

## Publish a version

The easiest path is **Actions → Release → Run workflow**, selecting **main** and
entering a stable version such as **0.1.0**, without a leading v. From the CLI:

~~~sh
gh workflow run release.yml --repo richiemcilroy/chippytea --ref main -f version=0.1.0
~~~

After validation, approve the **release** environment deployment in the run.
Check the requested version and source commit before approving: this releases
the signing secrets to that exact job. The configured reviewer is richiemcilroy;
self-approval is allowed so the solo maintainer can publish.

Alternatively, push a tag such as v0.1.0 pointing at a reviewed commit on main.
Tagged commits must belong to main's history. Manual releases cannot run from a
feature branch. The source commit is fixed at dispatch; the job does not build a
later main commit if main advances while the release runs.

Both CFBundleShortVersionString and CFBundleVersion are set from the requested
version. Versions must be canonical X.Y.Z numbers, strictly newer than every
published stable release. Prerelease suffixes, build suffixes, leading zeros,
and retargeted tags are rejected. Apple's bundle-version field limits are
enforced: major at most 9999; minor and patch at most 99.

Manual runs create the version tag only after local packaging and validation
succeed. An existing tag must already resolve to the exact source commit.
Never reuse a published version, move its tag, or replace its assets.

The stable release concurrency group does not cancel a running release.
GitHub retains at most one pending run in that group, so queue one release at a
time; if several versions are dispatched together, a superseded pending run
may need dispatching again.

## Credentials and access

The workflow uses the **release** GitHub environment, restricted to main and
v* tags, with required maintainer approval before secrets are exposed. Review
workflow changes before approving a release. The repository's immutable-release
tag ruleset blocks updates and deletions of v* tags without bypass actors. New
tags remain creatable so the approved workflow can create its version tag.
Main-branch review and required-check rules should be maintained separately.

The validation job has only contents:read. The release job needs contents:write
to create a tag, upload draft assets, and publish the release. It does not need
a personal access token: the job's GitHub token is sufficient.

| Name | Where | Purpose |
| --- | --- | --- |
| APPLE_CERTIFICATE_P12_BASE64 | Secret | Single-line base64 of the Developer ID Application certificate **with its private key**, exported as a password-protected P12. |
| APPLE_CERTIFICATE_PASSWORD | Secret | Password protecting the P12. |
| APPLE_SIGNING_IDENTITY | Variable or secret | Exact Developer ID Application identity, including its Apple team suffix. |
| APPLE_TEAM_ID | Variable or secret | The 10-character Apple Developer team ID. |
| SPARKLE_PRIVATE_KEY | Secret | Base64 of the 32-byte Sparkle Ed25519 private seed matching native/Info.plist. |
| APPLE_ID | Secret | Apple Developer account used for notarization. |
| APPLE_APP_SPECIFIC_PASSWORD | Secret | App-specific password for that account, not its normal sign-in password. |

Instead of the two Apple ID secrets, a team App Store Connect API key may be
used. Provide all three of ASC_KEY_ID, ASC_ISSUER_ID, and ASC_PRIVATE_KEY as
secrets; the last value is the complete P8 file with real newlines. A partial
API-key configuration fails rather than falling back to another identity.
Individual API keys without an issuer are not supported by this workflow.

Install secret values using GitHub's secret UI or standard input to gh secret
set. Do not put them in shell arguments, commits, issue comments, release notes,
or build artifacts. Apple ID credentials and an app-specific password cannot
be manufactured by the build; an authorized Apple Developer account must
supply them.

The signing certificate is imported into a temporary keychain. Signing and
notarization refer to that keychain explicitly; the workflow does not select
a new default keychain or replace the user's keychain search list. A cleanup trap removes the
keychain and temporary secret files even if the build fails.

### Preserve the Sparkle key

The public key in native/Info.plist is not secret. The private seed must match
it; the pipeline derives the public key with CryptoKit and checks the exact
app being packaged. Keep an encrypted, access-controlled backup of that seed.

Do **not** generate a new Sparkle key for each release. Installed applications
trust the existing key and require signed feeds as well as signed downloads.
Losing it can prevent those applications from accepting future updates.
Plan a key migration with a signed bridging release before changing it.
Renewing an expiring Apple signing certificate does not require changing the
Sparkle key.

## What the workflow verifies

1. The repository, main ancestry, exact source SHA, tag, version order, and
   current public update channel agree.
2. Offline release-boundary tests, Rust tests, and the built app's native and
   update-policy self-tests pass.
3. Both architectures build. The app carries its expected bundle identifier,
   versions, macOS requirement, feed URL, and Sparkle public key.
4. The app has a Developer ID signature from the configured team, a secure
   timestamp, and Hardened Runtime.
5. Apple accepts the app; its ticket is stapled. The ZIP is created **after**
   stapling, preserving framework symlinks. Its paths, metadata, and checksums
   are validated before extraction, then the recovered app passes codesign,
   stapler, and Gatekeeper assessment.
6. The APFS DMG contains Chippytea.app and an Applications shortcut. The DMG
   itself is signed, notarized, stapled, and Gatekeeper-assessed. Its mounted,
   read-only copy of the app is checked again.
7. The pinned Sparkle distribution produces the update enclosure and signed
   feed. Sparkle verifies both signatures; the enclosure URL and size must
   match the exact versioned ZIP.
8. All assets are uploaded to a **draft** and downloaded again to verify their
   bytes. The previous public feed and version/tag checks run again immediately
   before one API request publishes the draft and marks it latest.
9. The public latest-feed URL must return the exact signed bytes that were
   verified locally.

Sparkle tools and framework versions are pinned in scripts/release/sparkle.json;
the downloaded tools archive must match its SHA-256. Update the dependency,
pin, and compatibility checks together when upgrading Sparkle.

SwiftPM uses separate arm64 and x86_64 scratch directories under
CHIPPYTEA_SWIFT_BUILD_PATH (or .build locally), so incremental universal builds
do not reuse another architecture's build database.

CI and release jobs both select Xcode 26.2 explicitly. Update that selection in
both workflows together and verify the new toolchain before publishing.

The resulting release contains:

| Asset | Use |
| --- | --- |
| Chippytea-X.Y.Z-universal.dmg | New installations; drag Chippytea to Applications. |
| Chippytea-X.Y.Z-universal.zip | Signed and notarized application consumed by Sparkle. |
| appcast.xml | Signed stable update history with embedded release notes. |
| release-notes.md | GitHub-generated notes bound to the source SHA. |
| release.json | Version, source commit, supported architectures, and update channel. |
| SHA256SUMS | SHA-256 for the five assets above. |

Verified artifacts are also retained temporarily on the Actions run. Secret
files and signing keychains are never included. On failure, notarization JSON
results are retained separately to help locate a rejected Apple submission.

## Keep the update channel intact

The app's stable feed is
[appcast.xml on the latest release](https://github.com/richiemcilroy/chippytea/releases/latest/download/appcast.xml).
Every enclosure uses its immutable, version-specific GitHub download URL.
The latest release's appcast is fetched and signature-checked before adding
the new version. All prior entries are retained and their version, URL, size,
signature, and minimum macOS requirement are checked for accidental changes.

Do not delete older release ZIPs: an older Mac may still need its last
compatible version. When raising the minimum macOS version, deliberately
update the app and release validation together, leaving historical entries
and their downloads intact. A previous stable release without an appcast stops
the pipeline; repairing that history requires a deliberate migration, not a
silent empty-feed fallback.

Full ZIP updates are used initially; the workflow does not generate binary
deltas. Embedded release notes travel inside the signed feed, avoiding a
separate unsigned notes endpoint.

## Failures and retries

A build, test, signing, notarization, upload, or prepublication verification
failure leaves the public latest release unchanged. Rerun the **original
failed Actions run** to keep the same source commit. A source-bound draft from
that run may be rebuilt and its unpublished assets replaced automatically;
unrelated drafts and published releases are never overwritten.

If a tag or the public feed changes while the release runs, start a fresh run
against the correct source/history. Do not force the old plan through.

If publication succeeds but the final public-download check fails, the log
explicitly says the release **was published**. Inspect the published release
and GitHub/CDN status. Do not rerun with new bytes under the same version or
automatically roll users back. Fix a bad public release with a higher version.

The tests can be run without secrets:

~~~sh
python3 -B -m unittest discover -s scripts/release -p test_release.py -v
bash -n scripts/release/build-release.sh
~~~

These checks do not prove that Apple accepted a release or that a real
installed app updated. Before considering the first release/update pair
validated, install release A from its public DMG, launch it from Applications,
publish release B, check for updates, download and install through Chippytea,
and verify the relaunched version and existing user data. Test a failed or
offline check as well. Perform invalid-signature testing on a separate test
feed; never alter production assets to simulate corruption.

## Reference documentation

- [Sparkle: publishing an update](https://sparkle-project.org/documentation/publishing/)
  describes signing, archives, embedded notes, and compatible update history.
- [Apple: customizing notarization](https://developer.apple.com/documentation/security/customizing-the-notarization-workflow)
  describes submission and stapling.
- [GitHub: release API](https://docs.github.com/en/rest/releases/releases)
  documents draft publication and the latest-release field.
