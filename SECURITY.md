# Security

Chippytea reads filesystem metadata and can permanently remove files after a cleanup action. Reports about authorization bypasses, path or symlink races, unsafe deletion, unintended access and exposure of local data are especially useful.

## Reporting a vulnerability

Do not put exploit details, private files or credentials in a public issue or pull request.

Use [GitHub's private vulnerability reporting](https://github.com/richiemcilroy/chippytea/security/advisories/new), also available under **Security → Report a vulnerability**.

Include the following privately:

- The affected commit or app version, macOS version and filesystem type.
- What access an attacker needs and what could happen.
- Minimal reproduction steps using newly created, disposable files.
- A suggested fix, if you have one.

Never demonstrate a deletion bug against real user data. If you accidentally expose a credential, revoke or rotate it first. Removing it from a later commit does not remove it from Git history.

## Scope and support

This repository is under active development. Report issues against the latest source and identify the exact commit you tested. There is no published long-term support schedule or guaranteed response time.

Local development builds are not evidence of distribution signing or notarization. Do not bypass macOS security controls to run an unfamiliar binary.

## Keep private data private

The app's local library can contain filesystem paths, permissions, cleanup history and folder bookmarks. Do not attach it to public reports. Review screenshots, terminal output and benchmark results for personal filenames before sharing them.

Keep `.env` files, tokens, signing keys, certificate exports and local signing configuration out of Git. A `.gitignore` rule does not protect a file that has already been committed.

The [recommendation policy](docs/SUGGESTIONS.md) documents current exclusions and known limits. It is a description of the safeguards, not a guarantee that every cleanup is risk-free.
