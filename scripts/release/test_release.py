"""Offline tests for the release boundary, not proof of Apple/Sparkle signing."""

import base64
import io
import json
from pathlib import Path
import plistlib
import shutil
import stat
import tempfile
import unittest
from unittest.mock import patch
import xml.etree.ElementTree as ET
import zipfile

import release


SHA = "a" * 40
SIGNATURE = base64.b64encode(b"s" * 64).decode()
ET.register_namespace("sparkle", release.SPARKLE[1:-1])


def plan(version="0.2.0"):
    return {"schema": 1, "repository": release.REPOSITORY, "version": version,
            "tag": f"v{version}", "sha": SHA, "event": "workflow_dispatch", "previous": None}


def app_info(version="0.2.0"):
    return {"CFBundleIdentifier": "app.chippytea.mac", "CFBundleVersion": version,
            "CFBundleShortVersionString": version, "LSMinimumSystemVersion": "14.0",
            "SUFeedURL": release.FEED_URL, "SURequireSignedFeed": True,
            "SUVerifyUpdateBeforeExtraction": True,
            "SUPublicEDKey": base64.b64encode(b"p" * 32).decode()}


def feed(path, versions=("0.2.0",), length=8, mutate=None):
    root = ET.Element("rss", version="2.0")
    channel = ET.SubElement(root, "channel")
    for version in versions:
        item = ET.SubElement(channel, "item")
        ET.SubElement(item, release.SPARKLE + "version").text = version
        ET.SubElement(item, release.SPARKLE + "shortVersionString").text = version
        ET.SubElement(item, release.SPARKLE + "minimumSystemVersion").text = "14.0"
        ET.SubElement(item, "enclosure", {
            "url": f"{release.RELEASE_URL}/download/v{version}/chippytea-{version}-universal.zip",
            "length": str(length), "type": "application/octet-stream",
            release.SPARKLE + "edSignature": SIGNATURE})
    if mutate:
        mutate(root)
    ET.ElementTree(root).write(path, encoding="utf-8", xml_declaration=True)


def make_zip(path, entries=(), info=None):
    with zipfile.ZipFile(path, "w") as archive:
        archive.writestr("chippytea.app/Contents/Info.plist", plistlib.dumps(info or app_info()))
        archive.writestr("chippytea.app/Contents/MacOS/chippytea", b"fixture executable")
        for name, value, mode in entries:
            entry = zipfile.ZipInfo(name)
            entry.create_system = 3
            entry.external_attr = mode << 16
            archive.writestr(entry, value)


class VersionTests(unittest.TestCase):
    def test_numeric_not_lexicographic_order(self):
        self.assertGreater(release.version_tuple("1.10.0"), release.version_tuple("1.9.99"))
        self.assertEqual(release.version_tuple("0.1.0"), (0, 1, 0))
        self.assertEqual(release.version_tuple("9999.99.99"), (9999, 99, 99))

    def test_rejects_noncanonical_unsafe_and_non_bundle_versions(self):
        for value in ("", "v1.2.3", "01.2.3", "1.02.3", "1.2.03", "1.2",
                      "1.2.3.4", "1.2.3-beta", "1.2.3+build", "1.2.3\n",
                      "1.2.$(id)", "1.2.3;id", "１.2.3", "10000.0.0", "1.100.0"):
            with self.subTest(value=value), self.assertRaises(release.ReleaseError):
                release.version_tuple(value)

    def test_manual_runs_only_on_main(self):
        env = {"GITHUB_REPOSITORY": release.REPOSITORY, "GITHUB_EVENT_NAME": "workflow_dispatch",
               "GITHUB_REF": "refs/heads/main", "RELEASE_INPUT_VERSION": "0.2.0"}
        self.assertEqual(release.requested_release(env), ("0.2.0", "v0.2.0"))
        for ref in ("refs/heads/feature", "refs/tags/v0.2.0"):
            with self.subTest(ref=ref), self.assertRaises(release.ReleaseError):
                release.requested_release(dict(env, GITHUB_REF=ref))

    def test_push_tag_and_repository_are_validated(self):
        env = {"GITHUB_REPOSITORY": release.REPOSITORY, "GITHUB_EVENT_NAME": "push",
               "GITHUB_REF": "refs/tags/v0.2.0"}
        self.assertEqual(release.requested_release(env), ("0.2.0", "v0.2.0"))
        for changes in ({"GITHUB_REF": "refs/tags/v0.2.0-rc1"},
                        {"GITHUB_REF": "refs/heads/main"},
                        {"GITHUB_REPOSITORY": "someone/fork"},
                        {"GITHUB_EVENT_NAME": "pull_request"}):
            with self.subTest(changes=changes), self.assertRaises(release.ReleaseError):
                release.requested_release(dict(env, **changes))

    def test_downgrades_and_republishing_fail_across_all_stable_history(self):
        history = [{"draft": False, "prerelease": False, "tag_name": "v1.9.0"},
                   {"draft": False, "prerelease": False, "tag_name": "v1.10.0"}]
        for candidate in ("1.8.0", "1.9.1", "1.10.0"):
            with self.subTest(candidate=candidate), self.assertRaises(release.ReleaseError):
                release.check_monotonic(candidate, history)
        release.check_monotonic("1.10.1", history)

    def test_unrelated_prereleases_do_not_move_stable_channel(self):
        release.check_monotonic("0.2.0", [
            {"draft": False, "prerelease": True, "tag_name": "v9.0.0-beta"},
            {"draft": True, "prerelease": False, "tag_name": "v0.2.0"}])
        with self.assertRaises(release.ReleaseError):
            release.check_monotonic("0.2.0", [
                {"draft": False, "prerelease": True, "tag_name": "v0.2.0"}])

    def test_missing_historical_feed_is_not_silently_discarded(self):
        previous = {"id": 1, "tag_name": "v0.1.0", "draft": False,
                    "prerelease": False, "assets": []}
        with self.assertRaises(release.ReleaseError):
            release.release_snapshot(previous)
        previous["assets"] = [{"id": 2, "name": "appcast.xml", "size": 100}]
        self.assertEqual(release.release_snapshot(previous)["feed_asset_id"], 2)

    def test_latest_cannot_skip_a_newer_stable_release(self):
        history = [{"draft": False, "prerelease": False, "tag_name": "v0.2.0"}]
        for previous in (None, {"tag": "v0.1.0"}):
            with self.subTest(previous=previous), self.assertRaises(release.ReleaseError):
                release.check_latest_history(history, previous)
        release.check_latest_history(history, {"tag": "v0.2.0"})

    def test_tag_mismatch_blocks_plan_before_build(self):
        environment = {"GITHUB_REPOSITORY": release.REPOSITORY, "GITHUB_EVENT_NAME": "push",
                       "GITHUB_REF": "refs/tags/v0.2.0", "GITHUB_SHA": SHA}
        with patch.object(release, "run") as run, patch.object(release, "api") as api, \
                patch.object(release, "all_releases", return_value=[]), \
                patch.object(release, "remote_tag_sha", return_value="b" * 40):
            run.return_value.stdout = SHA + "\n"
            api.return_value = {"default_branch": "main", "private": False}
            with self.assertRaisesRegex(release.ReleaseError, "another commit"):
                release.make_plan(environment)


class TemporaryTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="chippytea-release-test-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)


class AppcastTests(TemporaryTests):
    def setUp(self):
        super().setUp()
        self.path = self.root / "appcast.xml"
        self.zip = self.root / "app.zip"
        self.zip.write_bytes(b"zipbytes")

    def test_current_and_all_historical_versions_are_preserved(self):
        previous = self.root / "previous.xml"
        feed(previous, ("0.1.0", "0.0.9"))
        feed(self.path, ("0.2.0", "0.1.0", "0.0.9"))
        self.assertEqual(release.validate_appcast(self.path, "0.2.0", self.zip, previous), SIGNATURE)
        feed(self.path, ("0.2.0", "0.1.0"))
        with self.assertRaisesRegex(release.ReleaseError, "removed or changed"):
            release.validate_appcast(self.path, "0.2.0", self.zip, previous)

    def test_historical_signature_and_minimum_os_cannot_change(self):
        previous = self.root / "previous.xml"
        feed(previous, ("0.1.0",))
        for tag, value in ((release.SPARKLE + "minimumSystemVersion", "15.0"),
                           (release.SPARKLE + "shortVersionString", "9.0.0")):
            feed(self.path, ("0.2.0", "0.1.0"),
                 mutate=lambda root: setattr(root.findall("channel/item")[1].find(tag), "text", value))
            with self.subTest(tag=tag), self.assertRaises(release.ReleaseError):
                release.validate_appcast(self.path, "0.2.0", self.zip, previous)

    def test_exact_https_versioned_asset_url_is_required(self):
        for url in ("http://github.com/richiemcilroy/chippytea/releases/download/v0.2.0/a.zip",
                    "https://github.com.evil.test/update.zip",
                    f"{release.RELEASE_URL}/latest/download/chippytea-0.2.0-universal.zip",
                    f"{release.RELEASE_URL}/download/v0.1.0/chippytea-0.2.0-universal.zip",
                    f"{release.RELEASE_URL}/download/v0.2.0/chippytea-0.2.0-universal.zip?x=1"):
            feed(self.path, mutate=lambda root: root.find("channel/item/enclosure").set("url", url))
            with self.subTest(url=url), self.assertRaises(release.ReleaseError):
                release.parse_appcast(self.path)

    def test_missing_malformed_or_truncated_signatures_fail(self):
        for signature in ("", "not base64!", base64.b64encode(b"s" * 63).decode()):
            feed(self.path, mutate=lambda root: root.find("channel/item/enclosure").set(
                release.SPARKLE + "edSignature", signature))
            with self.subTest(signature=signature), self.assertRaises(release.ReleaseError):
                release.parse_appcast(self.path)

    def test_mismatched_download_size_is_rejected(self):
        feed(self.path, length=9)
        with self.assertRaisesRegex(release.ReleaseError, "size differs"):
            release.validate_appcast(self.path, "0.2.0", self.zip)

    def test_duplicates_beta_channels_and_external_entities_fail(self):
        feed(self.path, ("0.2.0", "0.2.0"))
        with self.assertRaisesRegex(release.ReleaseError, "duplicate"):
            release.parse_appcast(self.path)
        feed(self.path, mutate=lambda root: ET.SubElement(
            root.find("channel/item"), release.SPARKLE + "channel"))
        with self.assertRaisesRegex(release.ReleaseError, "beta"):
            release.parse_appcast(self.path)
        self.path.write_text('<!DOCTYPE rss [<!ENTITY x SYSTEM "file:///etc/passwd">]><rss/>')
        with self.assertRaisesRegex(release.ReleaseError, "entity"):
            release.parse_appcast(self.path)

    def test_newest_version_must_match_the_release(self):
        feed(self.path, ("0.3.0", "0.2.0"))
        with self.assertRaisesRegex(release.ReleaseError, "not the newest"):
            release.validate_appcast(self.path, "0.2.0", self.zip)


class ArchiveTests(TemporaryTests):
    def setUp(self):
        super().setUp()
        self.path = self.root / "chippytea.zip"

    def test_framework_symlinks_are_preserved_and_accepted(self):
        base = "chippytea.app/Contents/Frameworks/Sparkle.framework"
        for target in ("Versions/Current/Sparkle", "versions/current/Sparkle"):
            make_zip(self.path, [
                (f"{base}/Versions/B/Sparkle", b"library", stat.S_IFREG | 0o755),
                (f"{base}/Versions/Current", "B", stat.S_IFLNK | 0o777),
                (f"{base}/Sparkle", target, stat.S_IFLNK | 0o777)])
            with self.subTest(target=target):
                release.validate_zip(self.path, "0.2.0")

    def test_traversal_absolute_and_unrelated_archive_entries_fail(self):
        for name in ("../escape", "/tmp/escape", "chippytea.app/../escape",
                     "Another.app/file", "chippytea.app\\..\\escape",
                     "__MACOSX/Other.app/resource"):
            make_zip(self.path, [(name, b"bad", stat.S_IFREG | 0o644)])
            with self.subTest(name=name), self.assertRaises(release.ReleaseError):
                release.validate_zip(self.path, "0.2.0")

    def test_absolute_parent_escaping_and_cyclic_symlinks_fail(self):
        for target in ("/tmp/escape", "../../../../escape", "link"):
            make_zip(self.path, [("chippytea.app/Contents/link", target, stat.S_IFLNK | 0o777)])
            with self.subTest(target=target), self.assertRaises(release.ReleaseError):
                release.validate_zip(self.path, "0.2.0")

    def test_symlink_chain_escape_not_visible_to_normpath_fails(self):
        make_zip(self.path, [
            ("chippytea.app/Contents/a", "..", stat.S_IFLNK | 0o777),
            ("chippytea.app/Contents/b", "a/../../escape", stat.S_IFLNK | 0o777)])
        with self.assertRaisesRegex(release.ReleaseError, "escapes"):
            release.validate_zip(self.path, "0.2.0")

    def test_case_and_normalization_folded_symlink_escapes_fail(self):
        for name, reference in (("a", "A"), ("\u00e9", "e\u0301")):
            make_zip(self.path, [
                (f"chippytea.app/Contents/{name}", "..", stat.S_IFLNK | 0o777),
                ("chippytea.app/Contents/b", f"{reference}/../..", stat.S_IFLNK | 0o777),
                ("chippytea.app/Contents/b/escape", b"bad", stat.S_IFREG | 0o644)])
            with self.subTest(name=name), self.assertRaisesRegex(release.ReleaseError, "escapes"):
                release.validate_zip(self.path, "0.2.0")

    def test_special_files_fail(self):
        make_zip(self.path, [("chippytea.app/Contents/pipe", b"", stat.S_IFIFO | 0o600)])
        with self.assertRaisesRegex(release.ReleaseError, "device or socket"):
            release.validate_zip(self.path, "0.2.0")

    def test_case_collisions_and_alias_overwrites_fail(self):
        for entries in (
                [("chippytea.app/contents/Info.plist", b"replacement", stat.S_IFREG | 0o644)],
                [("chippytea.app/Contents/alias", "MacOS", stat.S_IFLNK | 0o777),
                 ("chippytea.app/Contents/alias/chippytea", b"replacement", stat.S_IFREG | 0o755)]):
            make_zip(self.path, entries)
            with self.subTest(entries=entries), self.assertRaises(release.ReleaseError):
                release.validate_zip(self.path, "0.2.0")

    def test_wrong_bundle_version_feed_or_missing_key_fails(self):
        for key, value in (("CFBundleVersion", "0.1.0"), ("SUFeedURL", "https://example.com"),
                           ("SURequireSignedFeed", False), ("SUVerifyUpdateBeforeExtraction", False),
                           ("SUPublicEDKey", "")):
            make_zip(self.path, info=dict(app_info(), **{key: value}))
            with self.subTest(key=key), self.assertRaises(release.ReleaseError):
                release.validate_zip(self.path, "0.2.0")

    def test_corrupt_zip_is_rejected(self):
        self.path.write_bytes(b"not an archive")
        with self.assertRaises(release.ReleaseError):
            release.validate_zip(self.path, "0.2.0")


class PublicationTests(TemporaryTests):
    def setUp(self):
        super().setUp()
        self.plan = plan()
        self.events = []
        self.assets = []
        for index, name in enumerate(release.artifact_names("0.2.0")):
            if name != "release.json":
                (self.root / name).write_bytes(f"artifact-{index}".encode())
        (self.root / "release-notes.md").write_text(f"<!-- chippytea-source: {SHA} -->\n")
        release.finish_artifacts(self.plan, self.root)
        for index, path in enumerate(sorted(self.root.iterdir())):
            self.assets.append({"id": index + 100, "name": path.name, "size": path.stat().st_size,
                                "state": "uploaded"})
        self.history = []
        self.upload_failure = False
        self.corrupt_download = False

    def fake_api(self, path, method="GET", body=None, missing_ok=False):
        self.events.append((method, path, body))
        if path.startswith(f"repos/{release.REPOSITORY}/releases?per_page=100&page="):
            page = int(path.rsplit("=", 1)[1])
            return self.history[(page - 1) * 100:page * 100]
        if path.endswith("/releases/tags/v0.2.0"):
            return next((item for item in self.history
                         if item["tag_name"] == "v0.2.0" and not item["draft"]), None)
        if path.endswith("/releases") and method == "POST":
            created = {"id": 7, **body, "assets": []}
            self.history.append(created)
            return created
        if path.endswith("/releases/7") and method == "GET":
            return next(item for item in self.history if item["id"] == 7)
        if path.endswith("/releases/7") and method == "PATCH":
            if body.get("draft") is False:
                self.events.append("published")
            current = next(item for item in self.history if item["id"] == 7)
            current.update(body)
            return current
        if "/releases/assets/" in path and method == "DELETE":
            asset_id = int(path.rsplit("/", 1)[1])
            current = next(item for item in self.history if item["id"] == 7)
            current["assets"] = [asset for asset in current["assets"] if asset["id"] != asset_id]
            return None
        if path.endswith("/releases/latest"):
            return {"id": 7}
        raise AssertionError(f"Unexpected API: {method} {path}")

    def fake_upload(self, arguments):
        self.events.append("upload")
        if self.upload_failure:
            raise release.ReleaseError("Upload failed")
        current = next(item for item in self.history if item["tag_name"] == "v0.2.0")
        current["assets"] = self.assets

    def fake_download(self, asset_id, path, size):
        self.events.append("download")
        asset = next(asset for asset in self.assets if asset["id"] == asset_id)
        if self.corrupt_download:
            path.write_bytes(b"wrong")
        else:
            shutil.copyfile(self.root / asset["name"], path)

    def invoke(self):
        with patch.object(release, "check_plan_current"), \
                patch.object(release, "remote_tag_sha", return_value=SHA), \
                patch.object(release, "api", side_effect=self.fake_api), \
                patch.object(release, "run", side_effect=self.fake_upload), \
                patch("sys.stdout", new_callable=io.StringIO), \
                patch.object(release, "download_asset", side_effect=self.fake_download), \
                patch.object(release, "urlopen",
                             return_value=io.BytesIO((self.root / "appcast.xml").read_bytes())):
            release.publish(self.plan, self.root)

    def test_artifact_tampering_blocks_all_remote_mutations(self):
        (self.root / "appcast.xml").write_bytes(b"modified")
        with self.assertRaisesRegex(release.ReleaseError, "changed after verification"):
            self.invoke()
        self.assertEqual(self.events, [])

    def test_upload_failure_never_publishes_the_draft(self):
        self.upload_failure = True
        with self.assertRaisesRegex(release.ReleaseError, "Upload failed"):
            self.invoke()
        self.assertNotIn("published", self.events)

    def test_corrupt_downloaded_asset_never_publishes_the_draft(self):
        self.corrupt_download = True
        with self.assertRaisesRegex(release.ReleaseError, "Uploaded bytes differ"):
            self.invoke()
        self.assertNotIn("published", self.events)

    def test_published_or_foreign_draft_is_never_replaced(self):
        for draft, body in ((False, f"<!-- chippytea-source: {SHA} -->"), (True, "Someone else's draft")):
            self.events = []
            self.history = [{"id": 7, "tag_name": "v0.2.0", "draft": draft,
                             "prerelease": False, "body": body, "assets": []}]
            with self.subTest(draft=draft), self.assertRaisesRegex(release.ReleaseError, "Refusing"):
                self.invoke()
            self.assertTrue(all(event[0] == "GET" for event in self.events))

    def test_interrupted_source_bound_draft_is_resumed_from_paginated_history(self):
        self.history = [{"id": index + 10, "tag_name": f"other-{index}", "draft": True}
                        for index in range(100)]
        self.history.append({"id": 7, "tag_name": "v0.2.0", "draft": True, "prerelease": False,
                             "body": f"<!-- chippytea-source: {SHA} -->",
                             "assets": [{"id": 50, "name": "interrupted-upload.zip"}]})
        self.invoke()
        mutations = [event for event in self.events if isinstance(event, tuple) and event[0] != "GET"]
        self.assertFalse(any(event[0] == "POST" for event in mutations))
        self.assertIn(("DELETE", f"repos/{release.REPOSITORY}/releases/assets/50", None), mutations)
        self.assertIn("published", self.events)

    def test_ambiguous_drafts_are_rejected_before_remote_mutations(self):
        self.history = [{"id": identifier, "tag_name": "v0.2.0", "draft": True,
                         "prerelease": False, "body": f"<!-- chippytea-source: {SHA} -->",
                         "assets": []} for identifier in (7, 8)]
        with self.assertRaisesRegex(release.ReleaseError, "Multiple releases"):
            self.invoke()
        self.assertTrue(all(event[0] == "GET" for event in self.events))

    def test_draft_is_refreshed_before_source_and_publication_guards(self):
        listed = {"id": 7, "tag_name": "v0.2.0", "draft": True, "prerelease": False,
                  "body": f"<!-- chippytea-source: {SHA} -->", "assets": []}
        for changes in ({"draft": False}, {"body": "Someone else's draft"},
                        {"tag_name": "v0.3.0"}, {"prerelease": True}):
            self.events = []
            self.history = [dict(listed, **changes)]
            with self.subTest(changes=changes), patch.object(release, "all_releases", return_value=[listed]), \
                    self.assertRaisesRegex(release.ReleaseError, "Refusing"):
                self.invoke()
            self.assertTrue(all(event[0] == "GET" for event in self.events))

    def test_all_uploaded_bytes_are_checked_before_atomic_publication(self):
        self.invoke()
        published_index = self.events.index("published")
        self.assertEqual(self.events[:published_index].count("download"), len(self.assets))
        publishing = [event for event in self.events if isinstance(event, tuple)
                      and event[0] == "PATCH" and event[2].get("draft") is False]
        self.assertEqual(len(publishing), 1)
        self.assertEqual(publishing[0][2]["make_latest"], "true")


if __name__ == "__main__":
    unittest.main()
