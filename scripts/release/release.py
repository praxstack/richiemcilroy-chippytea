#!/usr/bin/env python3
"""Release validation and GitHub publication. Requires Python 3.12+ and gh."""

from __future__ import annotations

import argparse
import base64
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import plistlib
import re
import stat
import subprocess
import sys
import tarfile
import tempfile
import time
import unicodedata
from urllib.request import urlopen
import xml.etree.ElementTree as ET
import zipfile


REPOSITORY = "richiemcilroy/chippytea"
RELEASE_URL = f"https://github.com/{REPOSITORY}/releases"
FEED_URL = f"{RELEASE_URL}/latest/download/appcast.xml"
SPARKLE = "{http://www.andymatuschak.org/xml-namespaces/sparkle}"
VERSION_RE = re.compile(r"(0|[1-9][0-9]{0,3})\.(0|[1-9][0-9]?)\.(0|[1-9][0-9]?)")
SHA_RE = re.compile(r"[0-9a-f]{40}")
MAX_FEED_BYTES = 5 * 1024 * 1024


class ReleaseError(Exception):
    pass


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ReleaseError(message)


def version_tuple(value: str) -> tuple[int, int, int]:
    require(isinstance(value, str) and VERSION_RE.fullmatch(value) is not None,
            "Version must be stable X.Y.Z, without v, leading zeros, or suffixes "
            "(major <= 9999; minor and patch <= 99, per CFBundleVersion).")
    return tuple(int(part) for part in value.split("."))


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def run(arguments: list[str], **kwargs) -> subprocess.CompletedProcess:
    result = subprocess.run(arguments, capture_output=True, text=True, **kwargs)
    require(result.returncode == 0, f"{Path(arguments[0]).name} command failed "
            f"(exit {result.returncode}). No release was published by this command.")
    return result


def resolve_signing_identity(output: str, expected_name: str, team: str) -> str:
    """Resolve captured `security find-identity -v` output without exposing it."""
    require(isinstance(team, str) and re.fullmatch(r"[A-Z0-9]{10}", team) is not None,
            "Signing identity configuration has an invalid Apple team ID.")
    require(isinstance(expected_name, str) and re.fullmatch(
        r"Developer ID Application: [^\r\n\x00]+ \(" + re.escape(team) + r"\)",
        expected_name) is not None,
        "Signing identity configuration must name a Developer ID Application for the configured team.")
    identities = []
    counts = []
    for line in output.splitlines():
        if re.match(r"\s*\d+\)", line):
            entry = re.fullmatch(r'\s*\d+\)\s+([A-Fa-f0-9]{40})\s+"([^\r\n\x00]+)"\s*', line)
            require(entry is not None,
                    "Code-signing identity query returned a malformed or invalid identity entry.")
            identities.append((entry[1].upper(), entry[2]))
        else:
            count = re.fullmatch(r"\s*(\d+) valid identities found\s*", line)
            if count:
                counts.append(int(count[1]))
    require(counts == [len(identities)],
            "Code-signing identity query returned an inconsistent valid-identity list.")
    matches = {fingerprint for fingerprint, name in identities if name == expected_name}
    require(len(matches) <= 1,
            f"Signing identity is ambiguous ({len(matches)} valid matching fingerprints).")
    if matches:
        return next(iter(matches))
    same_team = {fingerprint for fingerprint, name in identities
                 if name.startswith("Developer ID Application: ") and name.endswith(f" ({team})")}
    if same_team:
        raise ReleaseError("Configured Developer ID label does not match imported valid identities "
                           f"(same-team identities: {len(same_team)}).")
    if identities:
        raise ReleaseError("No valid Developer ID identity for the configured team "
                           f"(valid identities: {len(set(identities))}).")
    raise ReleaseError("No valid code-signing identities in the isolated keychain "
                       "(valid identities: 0). Check the certificate/private-key pair and validity.")


def query_signing_identity(keychain: Path, environment: dict[str, str]) -> str:
    # Raw output may contain names/configuration stored as secrets. Never log it,
    # including when the process fails before an identity can be resolved.
    try:
        result = subprocess.run(
            ["security", "find-identity", "-v", "-p", "codesigning", str(keychain)],
            capture_output=True, text=True, timeout=30,
            env={**environment, "LC_ALL": "C"})
    except subprocess.TimeoutExpired:
        raise ReleaseError("Code-signing identity query timed out.") from None
    except (OSError, UnicodeError, subprocess.SubprocessError):
        raise ReleaseError("Code-signing identity query could not run.") from None
    require(result.returncode == 0,
            f"Code-signing identity query failed (exit {result.returncode}).")
    return resolve_signing_identity(result.stdout, environment.get("APPLE_SIGNING_IDENTITY", ""),
                                    environment.get("APPLE_TEAM_ID", ""))


def classify_signing_probe_error(output: str) -> str:
    # Only fixed category labels may escape the private log. Prefer a specific
    # reported failure over errSecInternalComponent when both appear.
    known = (
        ("KEYCHAIN_ITEM_NOT_FOUND", ("the specified item could not be found in the keychain",
                                     "errsecitemnotfound")),
        ("UNTRUSTED_CERTIFICATE_CHAIN", ("unable to build chain to self-signed root",
                                         "cssmerr_tp_not_trusted", "errsecnottrusted")),
        ("TIMESTAMP_SERVICE", ("the timestamp service is not available",
                               "a timestamp was expected but was not found")),
        ("ERR_SEC_INTERNAL_COMPONENT", ("errsecinternalcomponent",)),
    )
    folded = output.casefold()
    for category, messages in known:
        if any(message in folded for message in messages):
            return category
    return "UNKNOWN"


def validate_keychain_search_list(paths: list[str]) -> list[str]:
    require(isinstance(paths, list) and all(
        isinstance(path, str) and path.startswith("/") and
        not any(character in path for character in ("\x00", "\r", "\n")) for path in paths),
        "User keychain search list contains an invalid path.")
    return paths


def parse_keychain_search_list(output: str) -> list[str]:
    require("\x00" not in output and "\r" not in output,
            "User keychain search-list output is malformed.")
    paths = []
    for line in output.split("\n"):
        quoted = line.strip(" \t")
        if not quoted:
            continue
        require(len(quoted) >= 2 and quoted.startswith('"') and quoted.endswith('"'),
                "User keychain search-list output is malformed.")
        # Apple's security wraps each path without escaping its contents.
        # Preserve literal quotes/backslashes; subprocess arrays never eval them.
        paths.append(quoted[1:-1])
    return validate_keychain_search_list(paths)


def keychain_search_list_command(paths: list[str] | None = None) -> str:
    arguments = ["security", "list-keychains", "-d", "user"]
    if paths is not None:
        # An empty array deliberately sets an empty list; it is not a query.
        arguments.extend(["-s", *validate_keychain_search_list(paths)])
    try:
        result = subprocess.run(arguments, capture_output=True, text=True, timeout=30,
                                env={**os.environ, "LC_ALL": "C"})
    except (OSError, UnicodeError, subprocess.SubprocessError):
        raise ReleaseError("User keychain search-list command could not complete.") from None
    require(result.returncode == 0,
            f"User keychain search-list command failed (exit {result.returncode}).")
    # security can report path-open/get-path errors but return success after
    # omitting those entries. Never accept or restore a partial list silently.
    require(not result.stderr.strip(), "User keychain search-list command reported an error.")
    return result.stdout


def snapshot_keychain_search_list(path: Path) -> None:
    paths = parse_keychain_search_list(keychain_search_list_command())
    try:
        # Never replace the original snapshot after the search list changes.
        with path.open("x", encoding="utf-8") as destination:
            os.fchmod(destination.fileno(), 0o600)
            json.dump({"schema": 1, "keychains": paths}, destination)
    except (OSError, UnicodeError):
        raise ReleaseError("Could not save the private keychain search-list snapshot.") from None


def apply_keychain_search_list_snapshot(path: Path, prepend: Path | None = None) -> None:
    try:
        snapshot = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, ValueError):
        raise ReleaseError("Could not read the private keychain search-list snapshot.") from None
    require(isinstance(snapshot, dict) and set(snapshot) == {"schema", "keychains"} and
            type(snapshot["schema"]) is int and snapshot["schema"] == 1,
            "Invalid keychain search-list snapshot schema.")
    paths = validate_keychain_search_list(snapshot["keychains"])
    if prepend is not None:
        paths = validate_keychain_search_list([str(prepend), *paths])
    keychain_search_list_command(paths)
    require(parse_keychain_search_list(keychain_search_list_command()) == paths,
            "User keychain search-list update could not be verified.")


def api(path: str, method: str = "GET", body: dict | None = None,
        missing_ok: bool = False):
    arguments = ["gh", "api", "--method", method, "-H",
                 "Accept: application/vnd.github+json", "-H",
                 "X-GitHub-Api-Version: 2022-11-28", path]
    if body is not None:
        arguments.extend(["--input", "-"])
    result = subprocess.run(arguments, input=json.dumps(body) if body is not None else None,
                            capture_output=True, text=True)
    if result.returncode:
        if missing_ok and "(HTTP 404)" in result.stderr:
            return None
        # Do not print response bodies, environment, or credentials in CI logs.
        raise ReleaseError(f"GitHub API {method} {path} failed (exit {result.returncode}).")
    return json.loads(result.stdout) if result.stdout.strip() else None


def all_releases() -> list[dict]:
    releases = []
    for page in range(1, 101):
        batch = api(f"repos/{REPOSITORY}/releases?per_page=100&page={page}")
        require(isinstance(batch, list), "GitHub returned an invalid release list.")
        releases.extend(batch)
        if len(batch) < 100:
            return releases
    raise ReleaseError("Release history exceeded the safety limit; review it before publishing.")


def remote_tag_sha(tag: str) -> str | None:
    version_tuple(tag.removeprefix("v"))
    reference = api(f"repos/{REPOSITORY}/git/ref/tags/{tag}", missing_ok=True)
    if reference is None:
        return None
    obj = reference["object"]
    for _ in range(8):
        if obj["type"] == "commit":
            require(SHA_RE.fullmatch(obj["sha"]) is not None, "Invalid remote commit SHA.")
            return obj["sha"]
        require(obj["type"] == "tag" and SHA_RE.fullmatch(obj["sha"]) is not None,
                "Release tag does not resolve to a commit.")
        obj = api(f"repos/{REPOSITORY}/git/tags/{obj['sha']}")["object"]
    raise ReleaseError("Release tag has excessive annotation nesting.")


def release_snapshot(release: dict | None) -> dict | None:
    if release is None:
        return None
    require(not release["draft"] and not release["prerelease"], "Latest release is not stable.")
    tag = release["tag_name"]
    require(tag.startswith("v"), "Latest stable release has an unsupported tag.")
    version_tuple(tag[1:])
    feeds = [asset for asset in release["assets"] if asset["name"] == "appcast.xml"]
    require(len(feeds) == 1, "Previous stable release must contain exactly one signed appcast.xml. "
            "Do not silently start a new feed and strand installed versions.")
    feed = feeds[0]
    require(0 < feed["size"] <= MAX_FEED_BYTES, "Previous appcast has an invalid size.")
    return {"id": release["id"], "tag": tag, "feed_asset_id": feed["id"],
            "feed_size": feed["size"]}


def check_monotonic(version: str, releases: list[dict]) -> None:
    candidate = version_tuple(version)
    for release in releases:
        if release["draft"]:
            continue
        require(release["tag_name"] != f"v{version}",
                "This version is already published; release a higher version instead.")
        if release["prerelease"]:
            continue
        tag = release["tag_name"]
        require(tag.startswith("v"), "A stable release has an unsupported tag.")
        require(candidate > version_tuple(tag[1:]),
                f"Version {version} must be greater than every published stable release ({tag}).")


def check_latest_history(releases: list[dict], previous: dict | None) -> None:
    stable = [version_tuple(item["tag_name"][1:]) for item in releases
              if not item["draft"] and not item["prerelease"]]
    if stable:
        require(previous is not None and version_tuple(previous["tag"][1:]) == max(stable),
                "GitHub latest does not match the highest stable release. Repair the channel "
                "before publishing so newer update history is not lost.")
    else:
        require(previous is None, "Latest release disagrees with the stable release history.")


def requested_release(environment: dict[str, str]) -> tuple[str, str]:
    require(environment.get("GITHUB_REPOSITORY") == REPOSITORY,
            f"Releases may only run in {REPOSITORY}.")
    event, ref = environment.get("GITHUB_EVENT_NAME"), environment.get("GITHUB_REF", "")
    if event == "push":
        require(ref.startswith("refs/tags/v"), "Push releases require a vX.Y.Z tag.")
        version = ref[len("refs/tags/v"):]
    elif event == "workflow_dispatch":
        require(ref == "refs/heads/main", "Manual releases must run from main.")
        version = environment.get("RELEASE_INPUT_VERSION", "")
    else:
        raise ReleaseError("Only tagged pushes and manual main-branch releases are supported.")
    version_tuple(version)
    return version, f"v{version}"


def make_plan(environment: dict[str, str]) -> dict:
    version, tag = requested_release(environment)
    sha = environment.get("GITHUB_SHA", "")
    require(SHA_RE.fullmatch(sha) is not None, "GitHub did not supply a valid source SHA.")
    require(run(["git", "rev-parse", "HEAD"]).stdout.strip() == sha,
            "Checkout does not match the triggering source commit.")
    run(["git", "merge-base", "--is-ancestor", sha, "origin/main"])
    repository = api(f"repos/{REPOSITORY}")
    require(repository["default_branch"] == "main" and not repository["private"],
            "The update channel requires the public repository and its main branch.")
    releases = all_releases()
    check_monotonic(version, releases)
    target = remote_tag_sha(tag)
    require(target is None or target == sha, "Existing version tag points to another commit.")
    require(environment["GITHUB_EVENT_NAME"] != "push" or target == sha,
            "The triggering tag is missing or changed.")
    previous = release_snapshot(api(f"repos/{REPOSITORY}/releases/latest", missing_ok=True))
    check_latest_history(releases, previous)
    return {"schema": 1, "repository": REPOSITORY, "version": version, "tag": tag,
            "sha": sha, "event": environment["GITHUB_EVENT_NAME"], "previous": previous}


def read_plan(path: Path) -> dict:
    plan = json.loads(path.read_text())
    require(plan.get("schema") == 1 and plan.get("repository") == REPOSITORY,
            "Invalid release plan.")
    version_tuple(plan["version"])
    require(plan["tag"] == f"v{plan['version']}" and SHA_RE.fullmatch(plan["sha"]) is not None,
            "Release plan source or tag is invalid.")
    require(plan["event"] in ("push", "workflow_dispatch"), "Invalid release event.")
    return plan


def check_plan_current(plan: dict) -> None:
    require(run(["git", "rev-parse", "HEAD"]).stdout.strip() == plan["sha"],
            "Source checkout changed after preflight.")
    releases = all_releases()
    check_monotonic(plan["version"], releases)
    current = release_snapshot(api(f"repos/{REPOSITORY}/releases/latest", missing_ok=True))
    check_latest_history(releases, current)
    require(current == plan["previous"], "The published update feed changed during this build. "
            "Rerun the release so its history is included.")
    target = remote_tag_sha(plan["tag"])
    require(target is None or target == plan["sha"], "Release tag changed during this build.")
    require(plan["event"] != "push" or target == plan["sha"], "Triggering tag was deleted.")


def download_asset(asset_id: int, destination: Path, expected_size: int) -> None:
    require(isinstance(asset_id, int) and asset_id > 0, "Invalid release asset ID.")
    with destination.open("wb") as output:
        result = subprocess.run(["gh", "api", "-H", "Accept: application/octet-stream",
                                 f"repos/{REPOSITORY}/releases/assets/{asset_id}"],
                                stdout=output, stderr=subprocess.PIPE)
    require(result.returncode == 0 and destination.stat().st_size == expected_size,
            "Release asset download failed or returned an unexpected size.")


def prepare(plan: dict, archives: Path) -> None:
    archives.mkdir(parents=True, exist_ok=True)
    require(not any(archives.iterdir()), "Appcast staging directory must be empty.")
    previous = plan["previous"]
    if previous:
        download_asset(previous["feed_asset_id"], archives / "appcast.xml", previous["feed_size"])
        parse_appcast(archives / "appcast.xml")
    request = {"tag_name": plan["tag"], "target_commitish": plan["sha"]}
    if previous:
        request["previous_tag_name"] = previous["tag"]
    generated = api(f"repos/{REPOSITORY}/releases/generate-notes", "POST", request)
    notes = (f"# chippytea {plan['version']}\n\n"
             "Requires macOS 14 or later. Includes Apple silicon and Intel support.\n\n"
             f"{generated['body'].strip()}\n\n"
             "New installation: open the DMG and drag chippytea to Applications. "
             "Existing installation: choose **Check for Updates…** in chippytea.\n\n"
             f"<!-- chippytea-source: {plan['sha']} -->\n")
    (archives / f"chippytea-{plan['version']}-universal.md").write_text(notes)


def signature_bytes(value: str) -> bytes:
    try:
        signature = base64.b64decode(value, validate=True)
    except (ValueError, TypeError):
        raise ReleaseError("Update enclosure has an invalid Ed25519 signature.") from None
    require(len(signature) == 64, "Update enclosure must have a 64-byte Ed25519 signature.")
    return signature


def parse_appcast(path: Path) -> dict[str, dict]:
    require(0 < path.stat().st_size <= MAX_FEED_BYTES, "Appcast size is invalid.")
    data = path.read_bytes()
    require(b"<!DOCTYPE" not in data.upper() and b"<!ENTITY" not in data.upper(),
            "DTD and entity declarations are forbidden in appcasts.")
    try:
        root = ET.fromstring(data)
    except ET.ParseError:
        raise ReleaseError("Appcast is not valid XML.") from None
    require(root.tag == "rss" and len(root.findall("channel")) == 1,
            "Appcast must have one RSS channel.")
    entries = {}
    for item in root.findall("channel/item"):
        version = item.findtext(SPARKLE + "version", "")
        version_tuple(version)
        require(version not in entries, "Appcast contains duplicate versions.")
        require(item.findtext(SPARKLE + "shortVersionString") == version,
                "Appcast display version and build version differ.")
        enclosures = item.findall("enclosure")
        require(len(enclosures) == 1, "Each release must have one full update enclosure.")
        enclosure = enclosures[0]
        expected_url = f"{RELEASE_URL}/download/v{version}/chippytea-{version}-universal.zip"
        require(enclosure.get("url") == expected_url,
                "Update URL must point to its exact versioned GitHub ZIP asset.")
        signature = enclosure.get(SPARKLE + "edSignature", "")
        signature_bytes(signature)
        length = enclosure.get("length", "")
        require(re.fullmatch(r"[1-9][0-9]*", length) is not None, "Invalid update download length.")
        require(enclosure.get("type") in ("application/octet-stream", "application/zip"),
                "Update enclosure is not a ZIP download.")
        minimum = item.findtext(SPARKLE + "minimumSystemVersion", "")
        require(re.fullmatch(r"[0-9]+\.[0-9]+(?:\.[0-9]+)?", minimum) is not None,
                "Appcast must declare a minimum macOS version.")
        require(item.find(SPARKLE + "channel") is None,
                "Stable releases must not accidentally enter a beta channel.")
        entries[version] = {"url": expected_url, "signature": signature,
                            "length": int(length), "minimum": minimum}
    require(bool(entries), "Appcast has no installable updates.")
    return entries


def validate_appcast(path: Path, version: str, archive: Path,
                     previous: Path | None = None) -> str:
    entries = parse_appcast(path)
    require(version in entries, "New release is missing from the appcast.")
    require(max(map(version_tuple, entries)) == version_tuple(version),
            "New release is not the newest appcast entry.")
    current = entries[version]
    require(current["length"] == archive.stat().st_size, "Appcast archive size differs.")
    require(current["minimum"] == "14.0", "New release unexpectedly changed its minimum macOS.")
    if previous:
        historical = parse_appcast(previous)
        require(version not in historical, "This version is already present in the previous feed.")
        for old_version, old_entry in historical.items():
            require(entries.get(old_version) == old_entry,
                    f"Previous update {old_version} was removed or changed.")
    return current["signature"]


def safe_member_name(name: str) -> PurePosixPath:
    require(name and "\\" not in name and all(ord(char) >= 32 for char in name),
            "Archive contains an invalid path.")
    path = PurePosixPath(name)
    require(not path.is_absolute() and ".." not in path.parts, "Archive path escapes its root.")
    require(path.parts and path.parts[0] in ("chippytea.app", "__MACOSX"),
            "ZIP must contain only chippytea.app and its resource-fork metadata.")
    if path.parts[0] == "__MACOSX":
        require(len(path.parts) == 1 or path.parts[1] in ("chippytea.app", "._chippytea.app"),
                "ZIP contains unrelated resource-fork metadata.")
    return path


def archive_path_key(path: PurePosixPath) -> str:
    return unicodedata.normalize("NFD", str(path)).casefold()


def validate_zip(path: Path, version: str) -> None:
    """Validate paths before ditto extraction; preserve safe framework symlinks."""
    try:
        with zipfile.ZipFile(path) as archive:
            names, links, folded_names = {}, {}, set()
            total = 0
            for member in archive.infolist():
                name = safe_member_name(member.filename)
                folded = archive_path_key(name)
                require(name not in names and folded not in folded_names,
                        "ZIP contains duplicate or case-colliding paths.")
                names[name] = member
                folded_names.add(folded)
                require(not member.flag_bits & 1, "Encrypted release ZIPs are forbidden.")
                mode = stat.S_IFMT(member.external_attr >> 16)
                require(mode in (0, stat.S_IFREG, stat.S_IFDIR, stat.S_IFLNK),
                        "ZIP contains a special device or socket.")
                total += member.file_size
                require(total <= 2 * 1024**3, "ZIP exceeds the expanded-size limit.")
                if mode == stat.S_IFLNK:
                    require(member.file_size <= 4096, "ZIP symlink target is excessive.")
                    target = archive.read(member).decode("utf-8")
                    require(target and not target.startswith("/") and "\\" not in target
                            and all(ord(char) >= 32 for char in target),
                            "ZIP contains an unsafe symlink.")
                    links[folded] = PurePosixPath(target)
            # Resolve each link component, including chains and parent components.
            # A textual normpath check alone misses escapes through another symlink.
            resolved_names = set()
            for name in names:
                pending, resolved, followed = list(name.parts), [], 0
                while pending:
                    component = pending.pop(0)
                    if component == "..":
                        require(len(resolved) > 1, "ZIP symlink escapes chippytea.app.")
                        resolved.pop()
                    elif component != ".":
                        resolved.append(component)
                        link = links.get(archive_path_key(PurePosixPath(*resolved)))
                        if link is not None:
                            followed += 1
                            require(followed <= 40, "ZIP contains a cyclic symlink.")
                            resolved.pop()
                            pending = list(link.parts) + pending
                if archive_path_key(name) not in links:
                    destination = archive_path_key(PurePosixPath(*resolved))
                    require(destination not in resolved_names,
                            "ZIP paths collide through a symlink.")
                    resolved_names.add(destination)
            info_path = PurePosixPath("chippytea.app/Contents/Info.plist")
            executable = PurePosixPath("chippytea.app/Contents/MacOS/chippytea")
            scan_helper = PurePosixPath("chippytea.app/Contents/Helpers/chippytea-scan-helper")
            require(info_path in names and executable in names, "ZIP is missing the application.")
            require(scan_helper in names, "ZIP is missing the read-only scan helper.")
            require(archive_path_key(info_path) not in links and
                    archive_path_key(executable) not in links and
                    all(archive_path_key(component) not in links
                        for component in (scan_helper, *scan_helper.parents)),
                    "Application metadata, executable and scan helper must not be symlinks.")
            require(stat.S_IFMT(names[scan_helper].external_attr >> 16) == stat.S_IFREG and
                    names[scan_helper].external_attr >> 16 & 0o111 and
                    names[scan_helper].file_size > 0,
                    "The scan helper must be a nonempty regular executable.")
            require(names[info_path].file_size <= 1024 * 1024, "App plist is excessively large.")
            info = plistlib.loads(archive.read(names[info_path]))
            validate_plist(info, version)
            require(archive.testzip() is None, "ZIP checksum validation failed.")
    except (zipfile.BadZipFile, UnicodeDecodeError, plistlib.InvalidFileException) as error:
        raise ReleaseError(f"Invalid release ZIP: {type(error).__name__}.") from None


def validate_plist(info: dict, version: str) -> None:
    version_tuple(version)
    require(info.get("CFBundleIdentifier") == "app.chippytea.mac", "Incorrect app identifier.")
    require(info.get("CFBundleVersion") == version and
            info.get("CFBundleShortVersionString") == version, "App bundle version mismatch.")
    require(info.get("LSMinimumSystemVersion") == "14.0", "App minimum macOS must be 14.0.")
    require(info.get("SUFeedURL") == FEED_URL, "App points to the wrong update feed.")
    require(info.get("SURequireSignedFeed") is True, "App must require signed update feeds.")
    require(info.get("SUVerifyUpdateBeforeExtraction") is True,
            "App must verify update signatures before extraction.")
    try:
        key = base64.b64decode(info.get("SUPublicEDKey", ""), validate=True)
    except (TypeError, ValueError):
        key = b""
    require(len(key) == 32, "App is missing its valid Sparkle public key.")


def fetch_tools(destination: Path) -> Path:
    manifest = json.loads((Path(__file__).parent / "sparkle.json").read_text())
    version, url, expected = manifest["version"], manifest["url"], manifest["sha256"]
    require(re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", version) is not None,
            "Invalid Sparkle tools version.")
    require(url == f"https://github.com/sparkle-project/Sparkle/releases/download/"
            f"{version}/Sparkle-{version}.tar.xz" and
            re.fullmatch(r"[0-9a-f]{64}", expected) is not None, "Invalid Sparkle tools pin.")
    destination.mkdir(parents=True, exist_ok=True)
    require(not any(destination.iterdir()), "Sparkle tools directory must be empty.")
    downloaded = destination / "sparkle.tar.xz"
    with urlopen(url, timeout=60) as response, downloaded.open("wb") as output:
        require(response.url.startswith("https://"), "Sparkle download left HTTPS.")
        for block in iter(lambda: response.read(1024 * 1024), b""):
            output.write(block)
            require(output.tell() <= 256 * 1024**2, "Sparkle download exceeds its safety limit.")
    require(sha256(downloaded) == expected, "Sparkle distribution checksum mismatch.")
    extracted = destination / "distribution"
    extracted.mkdir()
    with tarfile.open(downloaded, "r:xz") as archive:
        require(sum(member.size for member in archive.getmembers()) <= 1024**3,
                "Sparkle distribution exceeds its expanded-size limit.")
        archive.extractall(extracted, filter="data")
    for name in ("generate_appcast", "sign_update"):
        require((extracted / "bin" / name).is_file(), f"Sparkle is missing {name}.")
    return extracted / "bin"


def artifact_names(version: str) -> list[str]:
    version_tuple(version)
    return [f"chippytea-{version}-universal.zip", f"chippytea-{version}-universal.dmg",
            "appcast.xml", "release-notes.md", "release.json"]


def finish_artifacts(plan: dict, directory: Path) -> None:
    manifest = {"version": plan["version"], "tag": plan["tag"], "source_commit": plan["sha"],
                "repository": REPOSITORY, "minimum_macos": "14.0",
                "architectures": ["arm64", "x86_64"], "update_feed": FEED_URL}
    (directory / "release.json").write_text(json.dumps(manifest, indent=2) + "\n")
    names = artifact_names(plan["version"])
    for name in names:
        require((directory / name).is_file() and not (directory / name).is_symlink(),
                f"Missing release artifact: {name}.")
    (directory / "SHA256SUMS").write_text("".join(
        f"{sha256(directory / name)}  {name}\n" for name in names))


def validate_artifacts(plan: dict, directory: Path) -> list[Path]:
    names = artifact_names(plan["version"])
    expected = "".join(f"{sha256(directory / name)}  {name}\n" for name in names)
    require((directory / "SHA256SUMS").read_text() == expected,
            "Release artifacts changed after verification.")
    manifest = json.loads((directory / "release.json").read_text())
    require(manifest["source_commit"] == plan["sha"] and manifest["version"] == plan["version"],
            "Artifacts belong to another source commit or version.")
    require({path.name for path in directory.iterdir()} == set(names + ["SHA256SUMS"]),
            "Artifact directory contains unexpected files; refusing to upload them.")
    files = [directory / name for name in names + ["SHA256SUMS"]]
    require(all(path.is_file() and not path.is_symlink() and path.stat().st_size > 0
                for path in files), "Release artifacts must be nonempty regular files.")
    return files


def publish(plan: dict, directory: Path) -> None:
    files = validate_artifacts(plan, directory)
    check_plan_current(plan)
    notes = (directory / "release-notes.md").read_text()
    marker = f"<!-- chippytea-source: {plan['sha']} -->"
    require(marker in notes, "Release notes are not bound to the source commit.")
    # The tag-specific REST endpoint only finds published releases, not drafts.
    matches = [item for item in all_releases() if item["tag_name"] == plan["tag"]]
    require(len(matches) <= 1,
            "Multiple releases share this version tag; refusing to choose a draft.")
    existing = None
    if matches:
        existing = api(f"repos/{REPOSITORY}/releases/{matches[0]['id']}")
        require(existing["tag_name"] == plan["tag"] and existing["draft"] and
                not existing["prerelease"] and marker in (existing.get("body") or ""),
                "Refusing to modify a published release or a draft owned by another source.")
    if remote_tag_sha(plan["tag"]) is None:
        require(plan["event"] == "workflow_dispatch", "Only manual releases may create a tag.")
        api(f"repos/{REPOSITORY}/git/refs", "POST",
            {"ref": f"refs/tags/{plan['tag']}", "sha": plan["sha"]})
    require(remote_tag_sha(plan["tag"]) == plan["sha"], "Release tag source could not be verified.")
    if existing:
        release = existing
        api(f"repos/{REPOSITORY}/releases/{release['id']}", "PATCH",
            {"name": f"chippytea {plan['version']}", "body": notes, "draft": True})
        # An interrupted run may leave a draft. Replace only this source-bound,
        # unpublished draft's assets, never the currently public release.
        for asset in release["assets"]:
            api(f"repos/{REPOSITORY}/releases/assets/{asset['id']}", "DELETE")
    else:
        release = api(f"repos/{REPOSITORY}/releases", "POST",
                      {"tag_name": plan["tag"], "target_commitish": plan["sha"],
                       "name": f"chippytea {plan['version']}", "body": notes,
                       "draft": True, "prerelease": False, "make_latest": "false"})
    run(["gh", "release", "upload", plan["tag"], "--repo", REPOSITORY,
         *[str(path) for path in files]])
    staged = api(f"repos/{REPOSITORY}/releases/{release['id']}")
    require(staged["draft"] and staged["tag_name"] == plan["tag"],
            "Draft release changed unexpectedly before publication.")
    assets = {asset["name"]: asset for asset in staged["assets"]}
    require(set(assets) == {path.name for path in files}, "Draft assets are incomplete or unexpected.")
    with tempfile.TemporaryDirectory(prefix="chippytea-upload-check-") as temp:
        for path in files:
            asset = assets[path.name]
            require(asset["state"] == "uploaded" and asset["size"] == path.stat().st_size,
                    f"Draft asset upload is incomplete: {path.name}.")
            recovered = Path(temp) / path.name
            download_asset(asset["id"], recovered, asset["size"])
            require(sha256(recovered) == sha256(path), f"Uploaded bytes differ: {path.name}.")
    check_plan_current(plan)
    # This is the only operation which makes the new feed/downloads public.
    # All assets already exist; publishing and advancing latest use one API call.
    try:
        published = api(f"repos/{REPOSITORY}/releases/{release['id']}", "PATCH",
                        {"draft": False, "prerelease": False, "make_latest": "true"})
    except ReleaseError:
        raise ReleaseError("Publication response is uncertain. Inspect the GitHub release before "
                           "retrying; it may already be public. Never replace published bytes.") from None
    require(not published["draft"], "GitHub did not confirm publication.")
    print(f"Published {RELEASE_URL}/tag/{plan['tag']}", flush=True)
    expected_feed = sha256(directory / "appcast.xml")
    for attempt in range(6):
        latest = api(f"repos/{REPOSITORY}/releases/latest")
        try:
            with urlopen(FEED_URL, timeout=20) as response:
                data = response.read(MAX_FEED_BYTES + 1)
            if latest["id"] == published["id"] and hashlib.sha256(data).hexdigest() == expected_feed:
                print("Public latest appcast matches the signed, verified release.")
                return
        except OSError:
            pass
        if attempt < 5:
            time.sleep(5)
    raise ReleaseError("Release WAS PUBLISHED, but public latest-feed verification failed. "
                       "Inspect GitHub/CDN visibility; do not replace the published version.")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    command = sub.add_parser("plan")
    command.add_argument("--output", type=Path, required=True)
    command.add_argument("--github-output", type=Path)
    command = sub.add_parser("tools")
    command.add_argument("--destination", type=Path, required=True)
    command = sub.add_parser("signing-identity")
    command.add_argument("--keychain", type=Path, required=True)
    command = sub.add_parser("signing-probe-error")
    command.add_argument("--log", type=Path, required=True)
    command = sub.add_parser("keychain-search-list")
    command.add_argument("action", choices=("snapshot", "prepend", "restore"))
    command.add_argument("--snapshot", type=Path, required=True)
    command.add_argument("--keychain", type=Path)
    command = sub.add_parser("prepare")
    command.add_argument("--plan", type=Path, required=True)
    command.add_argument("--archives", type=Path, required=True)
    command = sub.add_parser("validate-zip")
    command.add_argument("--archive", type=Path, required=True)
    command.add_argument("--version", required=True)
    command = sub.add_parser("validate-appcast")
    command.add_argument("--feed", type=Path, required=True)
    command.add_argument("--archive", type=Path, required=True)
    command.add_argument("--version", required=True)
    command.add_argument("--previous", type=Path)
    command = sub.add_parser("validate-plist")
    command.add_argument("--plist", type=Path, required=True)
    command.add_argument("--version", required=True)
    for name in ("finish", "publish"):
        command = sub.add_parser(name)
        command.add_argument("--plan", type=Path, required=True)
        command.add_argument("--artifacts", type=Path, required=True)
    args = parser.parse_args()
    if args.command == "plan":
        plan = make_plan(dict(os.environ))
        args.output.write_text(json.dumps(plan, indent=2) + "\n")
        if args.github_output:
            with args.github_output.open("a") as output:
                for key in ("version", "tag", "sha"):
                    output.write(f"{key}={plan[key]}\n")
        print(f"Validated {plan['tag']} at {plan['sha']}.")
    elif args.command == "tools":
        print(fetch_tools(args.destination))
    elif args.command == "signing-identity":
        print(query_signing_identity(args.keychain, dict(os.environ)))
    elif args.command == "signing-probe-error":
        try:
            output = args.log.read_text(encoding="utf-8")
        except (OSError, UnicodeError):
            output = ""
        print(classify_signing_probe_error(output))
    elif args.command == "keychain-search-list":
        require((args.action == "prepend") == (args.keychain is not None),
                "A keychain argument is required only when prepending to the search list.")
        if args.action == "snapshot":
            snapshot_keychain_search_list(args.snapshot)
        else:
            apply_keychain_search_list_snapshot(args.snapshot, args.keychain)
    elif args.command == "prepare":
        prepare(read_plan(args.plan), args.archives)
    elif args.command == "validate-zip":
        validate_zip(args.archive, args.version)
    elif args.command == "validate-appcast":
        print(validate_appcast(args.feed, args.version, args.archive, args.previous))
    elif args.command == "validate-plist":
        validate_plist(plistlib.loads(args.plist.read_bytes()), args.version)
    elif args.command == "finish":
        finish_artifacts(read_plan(args.plan), args.artifacts)
    elif args.command == "publish":
        publish(read_plan(args.plan), args.artifacts)


if __name__ == "__main__":
    try:
        main()
    except (ReleaseError, OSError, KeyError, ValueError, tarfile.TarError) as error:
        print(f"Release error: {error}", file=sys.stderr)
        sys.exit(1)
