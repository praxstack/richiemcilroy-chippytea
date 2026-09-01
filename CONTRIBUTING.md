# Contributing to Chippytea

Good bug reports, careful tests, small fixes and design work are all welcome. If a change affects what Chippytea can delete, open an issue first so we can agree on the safety rules.

## Getting started

Follow the [development guide](docs/development.md) to build the native app, run the tests or work on the landing page. The [architecture](docs/ARCHITECTURE.md) explains the engine; the [recommendation policy](docs/SUGGESTIONS.md) explains what belongs in the cleanup list.

Use new, disposable fixtures for every cleanup test. Never use your home folder, a working project, personal Downloads or someone else's files as test data.

## Reporting a bug

Search existing issues first. Include:

- The commit or app version, macOS version and Mac architecture.
- What you expected, what happened and the smallest steps that reproduce it.
- Relevant test output, or a screenshot made with synthetic data.

Replace personal paths, filenames and account details before posting. Do not upload your app database, folder bookmarks, credentials or a copy of your real files. Report suspected vulnerabilities through [the security policy](SECURITY.md), not a public bug report.

## Sending a pull request

Keep each PR focused. Explain the change, why it is needed and how you checked it. Say which checks you did not run; a successful build is not proof that cleanup or permission handling works.

For Rust or native changes, run the checks in the [development guide](docs/development.md#checks). Extend the existing tests when behavior changes. Safety fixes should include a disposable regression fixture and an assertion that files outside the operation survive. For interface changes, include before-and-after images in both light and dark mode when relevant. Respect reduced-motion settings.

Keep performance claims reproducible: name the hardware, exact build, fixture, commands and samples. Distinguish suggestion discovery from exhaustive metadata traversal, and first runs from controlled cold-cache measurements. See [performance notes](docs/PERFORMANCE.md).

Before committing, review both the working diff and the staged diff. Keep lockfiles with dependency changes. Leave generated builds, local benchmark output, signing material and `.env` files out of the commit. Use placeholders in configuration examples, never working credentials.

Run `python3 scripts/check-public-files.py` to check the file list. CI also scans file contents and Git history with Gitleaks. Neither replaces reviewing what you are about to publish.

## Project rules

- Cleanup stays within explicitly authorized folders and revalidates before mutation.
- A recommendation is not proof that a file is unwanted. Keep the consequence clear.
- Trash earns no chips. Pending estimates are not confirmed space credit.
- Preserve the native interface and the local-first design.

Contributions are made under the project's [MIT license](LICENSE). Be considerate of other contributors and follow the [code of conduct](CODE_OF_CONDUCT.md).
