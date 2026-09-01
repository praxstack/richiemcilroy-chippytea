#!/usr/bin/env python3
"""Reject private file types and local output before a public commit.

This checks paths, not secret contents. Run Gitleaks as well.
"""

import argparse
import subprocess
import sys
from pathlib import Path, PurePosixPath


PRIVATE_SUFFIXES = {
    ".pem", ".key", ".p8", ".p12", ".pfx", ".jks", ".keystore",
    ".keychain", ".keychain-db", ".mobileprovision", ".provisionprofile",
}
LOCAL_DIRECTORIES = {
    ".aws", ".ssh", ".codex", ".claude", ".direnv", ".swiftpm",
    ".vercel", ".next", "node_modules", "__pycache__",
}
LOCAL_ROOTS = {"target", ".build", "build", "research"}


def private_reason(name):
    path = PurePosixPath(name)
    leaf = path.name.lower()
    parts = tuple(part.lower() for part in path.parts)
    if leaf not in {".env.example", ".env.sample"} and (
        leaf == ".env" or leaf.startswith(".env.")
        or leaf.endswith(".env") or ".env." in leaf
    ):
        return "environment file"
    if path.suffix.lower() in PRIVATE_SUFFIXES:
        return "key, certificate export or signing file"
    if leaf == "sparkle-private-key" or leaf.startswith("sparkle-private-key."):
        return "private update-signing key"
    if leaf in {".envrc", ".npmrc", ".pypirc", "credentials.json"} or (
        leaf.startswith("service-account") and leaf.endswith(".json")
    ):
        return "local credential configuration"
    if any(part in LOCAL_DIRECTORIES for part in parts):
        return "local configuration or dependency output"
    if parts and parts[0] in LOCAL_ROOTS:
        return "local build or research output"
    if parts[:2] in {("benchmarks", "local"), ("docs", "local")}:
        return "private local evidence"
    if leaf == ".ds_store" or leaf.endswith((".log", ".xcuserstate")):
        return "local system state or log"
    return None


def self_test():
    blocked = [
        ".env", "site/.env.production", "nested/staging.env", "app.env.local",
        "signing/identity.P12", "private.key", "AuthKey_example.p8", ".npmrc",
        "site/.vercel/project.json", "research/report.md", "target/debug/app",
        "benchmarks/local/receipt.json", "docs/local/scan.json", "app.log",
        "site/node_modules/package/index.js", "credentials.json",
        "nested/.AWS/credentials", "nested/.SSH/id_ed25519", "DOCS/LOCAL/scan.json",
        "sparkle-private-key", "signing/Sparkle-Private-Key.txt",
    ]
    allowed = [
        ".env.example", "site/.env.sample", "README.md", "Cargo.lock",
        "Package.resolved", "site/bun.lock", "native/Info.plist",
        "docs/assets/readme-dark.svg", "scripts/release/sparkle.json",
    ]
    for name in blocked:
        assert private_reason(name), f"Must reject {name}"
    for name in allowed:
        assert private_reason(name) is None, f"Must allow {name}"
    print(f"Public-file rules: {len(blocked) + len(allowed)} cases passed.")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--self-test", action="store_true", help="test the path rules without reading files")
    args = parser.parse_args()
    if args.self_test:
        self_test()
        return 0

    root = Path(__file__).resolve().parents[1]
    result = subprocess.run(
        ["git", "ls-files", "--cached", "--others", "--exclude-standard", "-z"],
        cwd=root, check=True, stdout=subprocess.PIPE,
    )
    names = sorted(set(result.stdout.decode("utf-8", errors="surrogateescape").split("\0")) - {""})
    index = subprocess.run(
        ["git", "ls-files", "--stage", "-z"],
        cwd=root, check=True, stdout=subprocess.PIPE,
    )
    symlinks = set()
    for record in index.stdout.decode("utf-8", errors="surrogateescape").split("\0"):
        metadata, separator, name = record.partition("\t")
        if separator and metadata.split(" ", 1)[0] == "120000":
            symlinks.add(name)
    rejected = []
    for name in names:
        reason = private_reason(name)
        if name in symlinks or (root / name).is_symlink():
            reason = "symlink: review its target before publishing"
        if reason:
            rejected.append((name, reason))
    if rejected:
        for name, reason in rejected:
            print(f"REJECTED {name!r}: {reason}", file=sys.stderr)
        return 1
    print(f"Public-file paths: {len(names)} checked. Run Gitleaks to check contents.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
