//! Read-only review of exact obsolete-extension records owned by an editor.
//!
//! This provider deliberately does not sort versions, infer liveness, or expose
//! a removal operation.  A `.obsolete` entry is only useful as owner evidence;
//! active-version, rollback, activity, and personalized-extension policy still
//! require the editor itself and remain outside this review.

use crate::{
    model::Result,
    safety::{self, EntryMeta},
};
use serde::de::{self, Deserializer, MapAccess, Visitor};
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, os::fd::AsRawFd, path::Path, sync::atomic::AtomicBool};

const MAX_OBSOLETE_BYTES: usize = 64 * 1024;
const MAX_MARKERS: usize = 256;
const MAX_RECORDS: usize = 64;
const MAX_TEXT_BYTES: usize = 512;

/// These are fixed local roots only.  No editor configuration or arbitrary
/// Library path is discovered by this module.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Editor {
    Vscode,
    Cursor,
}

impl Editor {
    fn directory(self) -> &'static str {
        match self {
            Self::Vscode => ".vscode",
            Self::Cursor => ".cursor",
        }
    }
}

/// Metadata evidence for one exact true `.obsolete` marker.  Every record is
/// review-only and deliberately has no size, reclaimability, or mutation flag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct EditorRecord {
    pub directory: String,
    pub extension_id: String,
    pub version: String,
    pub marker: String,
    pub review_only: bool,
}

/// A bounded report. `complete == false` means the provider could not prove
/// that all marked records were understood; it never means removal is safe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct EditorReview {
    pub editor: Editor,
    pub extensions_root: String,
    pub marker_count: u64,
    pub verified_count: u64,
    pub records: Vec<EditorRecord>,
    pub complete: bool,
    pub reason: Option<String>,
}

struct ObsoleteVisitor;

impl<'de> Visitor<'de> for ObsoleteVisitor {
    type Value = Vec<(String, bool)>;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("an object mapping exact extension directory names to booleans")
    }

    fn visit_map<M>(self, mut map: M) -> std::result::Result<Self::Value, M::Error>
    where
        M: MapAccess<'de>,
    {
        let mut seen = HashSet::new();
        let mut entries = Vec::new();
        while let Some(key) = map.next_key::<String>()? {
            if !seen.insert(key.clone()) {
                return Err(de::Error::custom("duplicate .obsolete key"));
            }
            if entries.len() == MAX_MARKERS {
                return Err(de::Error::custom(".obsolete entry limit reached"));
            }
            let value = map.next_value::<bool>()?;
            entries.push((key, value));
        }
        Ok(entries)
    }
}

#[derive(Debug)]
struct PackageManifest {
    publisher: String,
    name: String,
    version: String,
}

struct PackageVisitor;

impl<'de> Visitor<'de> for PackageVisitor {
    type Value = PackageManifest;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("an extension package manifest")
    }

    fn visit_map<M>(self, mut map: M) -> std::result::Result<Self::Value, M::Error>
    where
        M: MapAccess<'de>,
    {
        let mut seen = HashSet::new();
        let mut publisher = None;
        let mut name = None;
        let mut version = None;
        while let Some(key) = map.next_key::<String>()? {
            if !seen.insert(key.clone()) {
                return Err(de::Error::custom("duplicate package.json key"));
            }
            match key.as_str() {
                "publisher" => publisher = Some(map.next_value::<String>()?),
                "name" => name = Some(map.next_value::<String>()?),
                "version" => version = Some(map.next_value::<String>()?),
                _ => {
                    let _: serde_json::Value = map.next_value()?;
                }
            }
        }
        Ok(PackageManifest {
            publisher: publisher.ok_or_else(|| de::Error::missing_field("publisher"))?,
            name: name.ok_or_else(|| de::Error::missing_field("name"))?,
            version: version.ok_or_else(|| de::Error::missing_field("version"))?,
        })
    }
}

fn parse_obsolete(bytes: &[u8]) -> std::result::Result<Vec<(String, bool)>, String> {
    parse_json_with_visitor(bytes, ObsoleteVisitor, ".obsolete")
}

fn parse_package(bytes: &[u8]) -> std::result::Result<PackageManifest, String> {
    parse_json_with_visitor(bytes, PackageVisitor, "package.json")
}

fn parse_json_with_visitor<'de, V>(
    bytes: &'de [u8],
    visitor: V,
    what: &str,
) -> std::result::Result<V::Value, String>
where
    V: Visitor<'de>,
{
    let mut decoder = serde_json::Deserializer::from_slice(bytes);
    let value = decoder
        .deserialize_any(visitor)
        .map_err(|error| format!("Invalid {what}: {error}"))?;
    decoder
        .end()
        .map_err(|error| format!("Invalid trailing data in {what}: {error}"))?;
    Ok(value)
}

fn bounded_text(value: &str) -> bool {
    value.len() <= MAX_TEXT_BYTES
        && !value.as_bytes().contains(&0)
        && !value.chars().any(char::is_control)
}

fn path_text(path: &Path) -> Option<String> {
    let value = path.to_str()?.to_owned();
    bounded_text(&value).then_some(value)
}

fn bounded_reason(value: String) -> String {
    if value.len() <= MAX_TEXT_BYTES {
        return value;
    }
    let mut end = MAX_TEXT_BYTES;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}

fn invalid_component(value: &str) -> bool {
    value.is_empty()
        || value == "."
        || value == ".."
        || !bounded_text(value)
        || value.contains('/')
        || value.contains('\\')
}

fn exact_extension_id_matches(marker: &str, package: &PackageManifest) -> bool {
    if invalid_component(marker)
        || invalid_component(&package.publisher)
        || invalid_component(&package.name)
        || invalid_component(&package.version)
    {
        return false;
    }
    let base = format!("{}.{}-{}", package.publisher, package.name, package.version);
    if marker == base {
        return true;
    }
    // These are explicit VS Code platform suffixes, not a generic prefix
    // match.  Unknown suffixes remain unverified.
    [
        "-darwin-arm64",
        "-darwin-x64",
        "-darwin-universal",
        "-linux-arm64",
        "-linux-x64",
        "-alpine-arm64",
        "-alpine-x64",
        "-win32-arm64",
        "-win32-x64",
        "-web",
    ]
    .iter()
    .any(|suffix| marker == format!("{base}{suffix}"))
}

fn directory_is_owned(meta: &EntryMeta) -> bool {
    // Directory link counts include `.` and one entry for every child
    // directory, so unlike package.json they are not expected to equal one.
    meta.is_dir() && !meta.is_dataless() && meta.links > 0 && meta.uid == unsafe { libc::geteuid() }
}

fn local_directory(path: &Path, cancel: &AtomicBool) -> Result<EntryMeta> {
    safety::cancelled(cancel)?;
    safety::validate_ancestors(path)?;
    let fd = safety::open_directory(path)?;
    safety::check_local(fd.as_raw_fd())?;
    let meta = safety::metadata(path)?;
    safety::cancelled(cancel)?;
    if !directory_is_owned(&meta) {
        return Err("The editor extensions directory is not an owned local directory".into());
    }
    Ok(meta)
}

fn missing(path: &Path) -> Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(false),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(error) => Err(format!("Cannot inspect editor review path: {error}")),
    }
}

fn read_bounded_regular(path: &Path, limit: usize, cancel: &AtomicBool) -> Result<Option<Vec<u8>>> {
    safety::cancelled(cancel)?;
    let meta = safety::metadata(path)?;
    if !meta.is_file()
        || meta.is_dataless()
        || meta.links != 1
        || meta.uid != unsafe { libc::geteuid() }
    {
        return Err("The editor review manifest is not an owned local regular file".into());
    }
    if meta.identity.size > limit as u64 {
        return Ok(None);
    }
    let file = safety::read_regular_bounded(path, cancel, limit as u64)?;
    Ok((file.bytes.len() <= limit).then_some(file.bytes))
}

fn partial(
    editor: Editor,
    extensions_root: &str,
    marker_count: u64,
    verified_count: u64,
    records: Vec<EditorRecord>,
    reason: impl Into<String>,
) -> EditorReview {
    EditorReview {
        editor,
        extensions_root: extensions_root.to_owned(),
        marker_count,
        verified_count,
        records,
        complete: false,
        reason: Some(bounded_reason(reason.into())),
    }
}

/// Inspect only the fixed default extension root for one editor.
///
/// The operation reads `.obsolete` and marked direct-child `package.json`
/// files. It never invokes the editor, follows links, scans sibling folders,
/// removes data, estimates bytes, or grants cleanup/coin eligibility.
pub(crate) fn inspect(home: &Path, editor: Editor, cancel: &AtomicBool) -> Result<EditorReview> {
    let _local_io = safety::LocalOnlyIo::new()?;
    safety::absolute_components(home)?;
    safety::cancelled(cancel)?;
    let extensions_root = home.join(editor.directory()).join("extensions");
    // Validate each fixed ancestor before treating a missing descendant as an
    // empty install. Otherwise a redirected `.vscode`/`.cursor` directory
    // could make a symlinked root look like a harmless absence.
    let _home_meta = local_directory(home, cancel)?;
    let editor_root = home.join(editor.directory());
    if missing(&editor_root)? {
        return Ok(EditorReview {
            editor,
            extensions_root: path_text(&extensions_root)
                .ok_or("The editor extensions path is too long or not UTF-8")?,
            marker_count: 0,
            verified_count: 0,
            records: Vec::new(),
            complete: true,
            reason: None,
        });
    }
    let _editor_meta = local_directory(&editor_root, cancel)?;
    let root_text =
        path_text(&extensions_root).ok_or("The editor extensions path is too long or not UTF-8")?;
    if missing(&extensions_root)? {
        return Ok(EditorReview {
            editor,
            extensions_root: root_text,
            marker_count: 0,
            verified_count: 0,
            records: Vec::new(),
            complete: true,
            reason: None,
        });
    }
    let root_before = local_directory(&extensions_root, cancel)?;
    let obsolete = extensions_root.join(".obsolete");
    if missing(&obsolete)? {
        if safety::metadata(&extensions_root).ok().as_ref() != Some(&root_before) {
            return Ok(partial(
                editor,
                &root_text,
                0,
                0,
                Vec::new(),
                "The editor extensions directory changed during review",
            ));
        }
        return Ok(EditorReview {
            editor,
            extensions_root: root_text,
            marker_count: 0,
            verified_count: 0,
            records: Vec::new(),
            complete: true,
            reason: None,
        });
    }
    let manifest = match read_bounded_regular(&obsolete, MAX_OBSOLETE_BYTES, cancel)? {
        Some(bytes) => bytes,
        None => {
            return Ok(partial(
                editor,
                &root_text,
                0,
                0,
                Vec::new(),
                ".obsolete exceeds the 64 KiB review bound",
            ));
        }
    };
    let markers = match parse_obsolete(&manifest) {
        Ok(markers) => markers,
        Err(reason) => return Ok(partial(editor, &root_text, 0, 0, Vec::new(), reason)),
    };
    let marked: Vec<String> = markers
        .into_iter()
        .filter_map(|(marker, obsolete)| obsolete.then_some(marker))
        .collect();
    let marker_count = marked.len() as u64;
    let mut records = Vec::new();
    let mut reason = None;
    for marker in marked {
        safety::cancelled(cancel)?;
        if records.len() == MAX_RECORDS {
            reason.get_or_insert("The review reached its 64-record bound".to_owned());
            continue;
        }
        if invalid_component(&marker) {
            reason.get_or_insert("An obsolete marker was not a direct child name".to_owned());
            continue;
        }
        let target = extensions_root.join(&marker);
        let before = match safety::metadata(&target) {
            Ok(meta) if directory_is_owned(&meta) => meta,
            Ok(_) => {
                reason.get_or_insert(
                    "A marked extension is not an owned ordinary directory".to_owned(),
                );
                continue;
            }
            Err(_) => {
                reason.get_or_insert(
                    "A marked extension directory is missing or inaccessible".to_owned(),
                );
                continue;
            }
        };
        let package_path = target.join("package.json");
        let package = match read_bounded_regular(&package_path, MAX_OBSOLETE_BYTES, cancel)? {
            Some(bytes) => match parse_package(&bytes) {
                Ok(package) => package,
                Err(_) => {
                    reason.get_or_insert(
                        "A marked extension has invalid package metadata".to_owned(),
                    );
                    continue;
                }
            },
            None => {
                reason.get_or_insert(
                    "A marked package.json exceeds the 64 KiB review bound".to_owned(),
                );
                continue;
            }
        };
        let after = match safety::metadata(&target) {
            Ok(meta) => meta,
            Err(_) => {
                reason.get_or_insert(
                    "A marked extension changed while its metadata was read".to_owned(),
                );
                continue;
            }
        };
        if before != after {
            reason
                .get_or_insert("A marked extension changed while its metadata was read".to_owned());
            continue;
        }
        if !exact_extension_id_matches(&marker, &package) {
            reason.get_or_insert(
                "A marked extension directory does not match package metadata".to_owned(),
            );
            continue;
        }
        let Some(directory) = path_text(&target) else {
            reason.get_or_insert("A marked extension path is not bounded UTF-8".to_owned());
            continue;
        };
        let extension_id = format!("{}.{}", package.publisher, package.name);
        if !bounded_text(&extension_id) {
            reason.get_or_insert("A marked extension identity exceeds the text bound".to_owned());
            continue;
        }
        records.push(EditorRecord {
            directory,
            extension_id,
            version: package.version,
            marker,
            review_only: true,
        });
    }
    safety::cancelled(cancel)?;
    if safety::metadata(&extensions_root).ok().as_ref() != Some(&root_before) {
        reason.get_or_insert("The editor extensions directory changed during review".to_owned());
    }
    let reason = reason.or_else(|| {
        (!records.is_empty()).then_some(
            "Ownership and package identity matched; active-version, rollback, activity, and personalized-directory policy were not validated".to_owned(),
        )
    });
    Ok(EditorReview {
        editor,
        extensions_root: root_text,
        marker_count,
        verified_count: records.len() as u64,
        records,
        complete: reason.is_none(),
        reason,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::symlink, path::PathBuf};
    use tempfile::TempDir;

    fn fixture(editor: Editor) -> (TempDir, PathBuf) {
        let temporary_root = std::env::temp_dir().canonicalize().unwrap();
        let home = tempfile::tempdir_in(temporary_root).unwrap();
        let extensions = home.path().join(editor.directory()).join("extensions");
        fs::create_dir_all(&extensions).unwrap();
        (home, extensions)
    }

    fn package(extensions: &Path, directory: &str, publisher: &str, name: &str, version: &str) {
        let target = extensions.join(directory);
        fs::create_dir_all(&target).unwrap();
        fs::write(
            target.join("package.json"),
            format!(
                "{{\"publisher\":\"{publisher}\",\"name\":\"{name}\",\"version\":\"{version}\",\"contributes\":{{}}}}"
            ),
        )
        .unwrap();
    }

    #[test]
    fn verifies_true_markers_without_touching_siblings() {
        let (home, extensions) = fixture(Editor::Vscode);
        package(
            &extensions,
            "vendor.editor-1.2.3",
            "vendor",
            "editor",
            "1.2.3",
        );
        package(
            &extensions,
            "vendor.editor-2.0.0",
            "vendor",
            "editor",
            "2.0.0",
        );
        package(
            &extensions,
            "vendor.editor-0.9.0",
            "vendor",
            "editor",
            "0.9.0",
        );
        package(
            &extensions,
            "vendor.editor-1.2.3-darwin-arm64",
            "vendor",
            "editor",
            "1.2.3",
        );
        fs::write(
            extensions.join(".obsolete"),
            r#"{"vendor.editor-1.2.3":true,"vendor.editor-1.2.3-darwin-arm64":true,"vendor.editor-0.9.0":false}"#,
        )
        .unwrap();

        let review = inspect(home.path(), Editor::Vscode, &AtomicBool::new(false)).unwrap();
        assert_eq!(review.marker_count, 2);
        assert_eq!(review.verified_count, 2);
        assert!(!review.complete);
        assert!(
            review
                .records
                .iter()
                .all(|record| record.extension_id == "vendor.editor")
        );
        assert!(
            review
                .records
                .iter()
                .all(|record| record.version == "1.2.3")
        );
        assert!(review.records.iter().all(|record| record.review_only));
        assert!(extensions.join("vendor.editor-2.0.0").exists());
        assert!(extensions.join("vendor.editor-0.9.0").exists());
    }

    #[test]
    fn rejects_ambiguous_manifests_and_unsafe_targets() {
        let cases = [
            r#"{"vendor.editor-1.0.0":true,"vendor.editor-1.0.0":false}"#,
            r#"{"vendor.editor-1.0.0":"true"}"#,
            r#"{"../outside":true}"#,
            r#"{"vendor.editor-1.0.0":true}"#,
        ];
        for (index, obsolete) in cases.iter().enumerate() {
            let (home, extensions) = fixture(Editor::Cursor);
            if index == cases.len() - 1 {
                symlink(
                    home.path().join("real-target"),
                    extensions.join("vendor.editor-1.0.0"),
                )
                .unwrap();
            }
            fs::write(extensions.join(".obsolete"), obsolete).unwrap();
            let review = inspect(home.path(), Editor::Cursor, &AtomicBool::new(false)).unwrap();
            assert!(!review.complete, "case {index} unexpectedly completed");
            assert!(review.records.is_empty(), "case {index} produced a record");
        }
    }

    #[test]
    fn ignores_false_markers_and_rejects_package_mismatch() {
        let (home, extensions) = fixture(Editor::Vscode);
        package(
            &extensions,
            "vendor.editor-1.2.3",
            "other",
            "editor",
            "1.2.3",
        );
        fs::write(
            extensions.join(".obsolete"),
            r#"{"not-a-path/1":false,"vendor.editor-1.2.3":true}"#,
        )
        .unwrap();
        let review = inspect(home.path(), Editor::Vscode, &AtomicBool::new(false)).unwrap();
        assert_eq!(review.marker_count, 1);
        assert_eq!(review.verified_count, 0);
        assert!(!review.complete);
    }

    #[test]
    fn bounds_markers_records_and_manifest_bytes() {
        let (home, extensions) = fixture(Editor::Vscode);
        let mut markers = String::from("{");
        for index in 0..257 {
            if index > 0 {
                markers.push(',');
            }
            markers.push_str(&format!("\"vendor.editor-{index}.0.0\":false"));
        }
        markers.push('}');
        fs::write(extensions.join(".obsolete"), markers).unwrap();
        let review = inspect(home.path(), Editor::Vscode, &AtomicBool::new(false)).unwrap();
        assert!(!review.complete);

        let (home, extensions) = fixture(Editor::Vscode);
        let mut markers = String::from("{");
        for index in 0..65 {
            if index > 0 {
                markers.push(',');
            }
            let version = format!("{index}.0.0");
            let directory = format!("vendor.editor-{version}");
            package(&extensions, &directory, "vendor", "editor", &version);
            markers.push_str(&format!("\"{directory}\":true"));
        }
        markers.push('}');
        fs::write(extensions.join(".obsolete"), markers).unwrap();
        let review = inspect(home.path(), Editor::Vscode, &AtomicBool::new(false)).unwrap();
        assert_eq!(review.records.len(), MAX_RECORDS);
        assert_eq!(review.verified_count, MAX_RECORDS as u64);
        assert!(!review.complete);

        let (home, extensions) = fixture(Editor::Cursor);
        fs::write(
            extensions.join(".obsolete"),
            vec![b' '; MAX_OBSOLETE_BYTES + 1],
        )
        .unwrap();
        let review = inspect(home.path(), Editor::Cursor, &AtomicBool::new(false)).unwrap();
        assert!(!review.complete);
        assert!(review.records.is_empty());
    }

    #[test]
    fn cancellation_stops_before_reading() {
        let (home, _) = fixture(Editor::Vscode);
        let cancel = AtomicBool::new(true);
        assert_eq!(
            inspect(home.path(), Editor::Vscode, &cancel).unwrap_err(),
            "Cancelled"
        );
    }
}
