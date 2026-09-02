//! Read-only project build-artifact evidence providers.
//!
//! These providers deliberately stop at ownership evidence.  They never invoke
//! a project manager or evaluate a manifest, and every result remains review /
//! Move-to-Trash only until the scanner's independent policy allows otherwise.
use crate::{
    model::{Identity, Result, Root},
    safety,
};
use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};

const RULE_VERSION: u32 = 1;
const MAX_PROVIDER_FILES: usize = 8;
const MAX_DIRECT_MARKERS: usize = 256;

/// Evidence returned to the scanner adapter.  The scanner remains responsible
/// for measurement, quiet-time policy, Git/activity checks and revalidation.
#[derive(Debug, Clone)]
pub(crate) struct ProviderEvidence {
    pub kind: &'static str,
    pub title: String,
    pub explanation: &'static str,
    pub consequence: &'static str,
    pub fingerprint: String,
    pub latest_modified_ns: i64,
    pub activity_root: PathBuf,
    pub blocked: Option<String>,
}

/// Exact names are only cheap dispatch.  `identify` still requires the
/// ecosystem-specific direct marker and current filesystem evidence.
pub(crate) fn recognizes_name(name: &OsStr) -> bool {
    matches!(
        name.to_str(),
        Some(".build" | ".dart_tool" | ".zig-cache" | "build" | ".gradle" | "bin" | "obj")
    )
}

#[derive(Debug)]
struct Capture {
    relative: String,
    identity: Identity,
    digest: blake3::Hash,
    explicit_override: bool,
    declares_flutter_sdk: bool,
    is_flutter_metadata: bool,
}

#[derive(Debug, Clone)]
struct Scope {
    project: PathBuf,
    artifact: safety::EntryMeta,
    project_meta: safety::EntryMeta,
}

fn present(path: &Path) -> Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("Cannot inspect project evidence: {error}")),
    }
}

fn capture(
    project: &Path,
    relative: &str,
    cancel: &AtomicBool,
    override_tokens: &[&[u8]],
) -> Result<Option<Capture>> {
    safety::cancelled(cancel)?;
    let path = project.join(relative);
    if !present(&path)? {
        return Ok(None);
    }
    // read_regular performs the owned, independent, bounded no-follow read and
    // validates the pathname again after reading.  A present non-regular marker
    // is therefore an error rather than a guessed project classification.
    let file = safety::read_regular(&path, cancel)?;
    let explicit_override = override_tokens.iter().any(|token| {
        file.bytes
            .windows(token.len())
            .any(|window| window == *token)
    });
    Ok(Some(Capture {
        relative: relative.into(),
        identity: file.identity,
        digest: blake3::hash(&file.bytes),
        explicit_override,
        declares_flutter_sdk: declares_flutter_sdk(&file.bytes),
        is_flutter_metadata: relative == ".metadata" && is_flutter_metadata(&file.bytes),
    }))
}

fn declares_flutter_sdk(bytes: &[u8]) -> bool {
    let mut dependencies = false;
    let mut flutter = false;
    for line in bytes.split(|byte| *byte == b'\n') {
        let leading = line.iter().take_while(|byte| **byte == b' ').count();
        let trimmed = &line[leading..];
        if leading == 0 {
            dependencies = trimmed == b"dependencies:";
            flutter = false;
            continue;
        }
        if dependencies && trimmed == b"flutter:" {
            flutter = true;
            continue;
        }
        if dependencies && flutter && leading > 2 && trimmed == b"sdk: flutter" {
            return true;
        }
        if dependencies && leading <= 2 {
            flutter = false;
        }
    }
    false
}

fn is_flutter_metadata(bytes: &[u8]) -> bool {
    let mut version = false;
    let mut project_type = false;
    for line in bytes.split(|byte| *byte == b'\n') {
        let leading = line.iter().take_while(|byte| **byte == b' ').count();
        let trimmed = &line[leading..];
        if leading == 0 && trimmed == b"version:" {
            version = true;
        }
        if leading == 0 && trimmed == b"project_type: app" {
            project_type = true;
        }
    }
    version && project_type
}

fn capture_any(
    project: &Path,
    names: &[&str],
    cancel: &AtomicBool,
    override_tokens: &[&[u8]],
) -> Result<Vec<Capture>> {
    let mut captures = Vec::new();
    for name in names {
        if let Some(captured) = capture(project, name, cancel, override_tokens)? {
            captures.push(captured);
        }
    }
    Ok(captures)
}

fn ensure_scope(root: &Root, artifact: &Path, cancel: &AtomicBool) -> Result<Option<Scope>> {
    safety::cancelled(cancel)?;
    safety::validate_root(root)?;
    safety::check_scope_policy(root, artifact)?;
    if artifact == root.path || !artifact.starts_with(&root.path) {
        return Err("The project artifact is outside its authorized folder".into());
    }
    let Some(artifact_meta) = safety::scope_metadata(root, artifact, cancel)? else {
        return Ok(None);
    };
    if !artifact_meta.is_dir() {
        // A symlink or regular file with a recognized basename is not an
        // artifact boundary.  No path-following fallback is permitted.
        return Ok(None);
    }
    if artifact_meta.identity.device != root.identity.device {
        return Err("The project artifact is on another storage volume".into());
    }
    let Some(project) = artifact.parent().map(Path::to_path_buf) else {
        return Ok(None);
    };
    let project_meta = safety::scope_metadata(root, &project, cancel)?
        .ok_or("The project artifact parent is unavailable")?;
    Ok(Some(Scope {
        project,
        artifact: artifact_meta,
        project_meta,
    }))
}

fn finish(
    root: &Root,
    artifact: &Path,
    scope: &Scope,
    cancel: &AtomicBool,
    kind: &'static str,
    captures: Vec<Capture>,
    blocked: Option<String>,
) -> Result<ProviderEvidence> {
    if captures.is_empty() || captures.len() > MAX_PROVIDER_FILES {
        return Err("Project ownership evidence is incomplete".into());
    }
    let Some(artifact_meta) = safety::scope_metadata(root, artifact, cancel)? else {
        return Err("The project artifact disappeared while evidence was captured".into());
    };
    let Some(project_meta) = safety::scope_metadata(root, &scope.project, cancel)? else {
        return Err("The project artifact parent disappeared while evidence was captured".into());
    };
    if artifact_meta != scope.artifact || project_meta != scope.project_meta {
        return Err("The project artifact changed while evidence was captured".into());
    }
    let mut hash = blake3::Hasher::new();
    hash.update(format!("project-provider-v{RULE_VERSION}:{kind}").as_bytes());
    hash.update(root.id.as_bytes());
    hash.update(artifact.as_os_str().as_encoded_bytes());
    hash.update(&artifact_meta.identity.device.to_le_bytes());
    hash.update(&artifact_meta.identity.inode.to_le_bytes());
    hash.update(&artifact_meta.identity.modified_ns.to_le_bytes());
    hash.update(&artifact_meta.identity.changed_ns.to_le_bytes());
    let mut latest = artifact_meta.identity.modified_ns;
    for captured in &captures {
        hash.update(captured.relative.as_bytes());
        hash.update(captured.digest.as_bytes());
        hash.update(&captured.identity.device.to_le_bytes());
        hash.update(&captured.identity.inode.to_le_bytes());
        hash.update(&captured.identity.mode.to_le_bytes());
        hash.update(&captured.identity.size.to_le_bytes());
        hash.update(&captured.identity.modified_ns.to_le_bytes());
        hash.update(&captured.identity.changed_ns.to_le_bytes());
        latest = latest.max(captured.identity.modified_ns);
    }
    let project_name = scope
        .project
        .file_name()
        .unwrap_or_else(|| OsStr::new("project"))
        .to_string_lossy();
    let (title, explanation, consequence) = match kind {
        "swiftpm" => (
            format!("{project_name} SwiftPM build artifacts"),
            "A direct Package.swift marker identifies this .build directory as Swift Package Manager output. This is a review and Move-to-Trash suggestion only.",
            "SwiftPM will rebuild the package and may resolve dependencies again. SwiftPM can use a custom --scratch-path, so this provider does not claim permanent ownership or coin eligibility.",
        ),
        "dart" => (
            format!("{project_name} Dart tool data"),
            "A direct pubspec.yaml marker identifies this .dart_tool directory as project-specific Dart tooling data. This is a review and Move-to-Trash suggestion only.",
            "Dart tooling will recreate project metadata and may resolve dependencies again. Global package caches are not included.",
        ),
        "flutter" => (
            format!("{project_name} Flutter build output"),
            "A direct pubspec.yaml Flutter SDK dependency or an exact Flutter .metadata marker identifies this build directory as Flutter-generated output. This is a review and Move-to-Trash suggestion only.",
            "Flutter will rebuild generated output; build configuration and custom output flags are not evaluated by this provider.",
        ),
        "zig" => (
            format!("{project_name} Zig build cache"),
            "A direct build.zig marker identifies this .zig-cache directory as local Zig build cache. This is a review and Move-to-Trash suggestion only.",
            "Zig will regenerate the cache. The user-selected zig-out installation prefix is deliberately not classified, and --cache-dir overrides are not inferred.",
        ),
        "gradle" => (
            format!("{project_name} Gradle build data"),
            "A direct Gradle settings or build script marker identifies this project-local build data. This is a review and Move-to-Trash suggestion only; global ~/.gradle data is excluded.",
            "Gradle will regenerate project outputs and caches. Build-directory overrides and plugin-generated outputs are not evaluated.",
        ),
        "dotnet" => (
            format!("{project_name} .NET build data"),
            "A direct .NET project marker identifies this bin/obj directory as project build data. This is a review and Move-to-Trash suggestion only.",
            ".NET will rebuild intermediate or final outputs. MSBuild output configuration is inspected only for explicit lexical warnings, never evaluated.",
        ),
        _ => return Err("Unknown project provider".into()),
    };
    Ok(ProviderEvidence {
        kind,
        title,
        explanation,
        consequence,
        fingerprint: hash.finalize().to_hex().to_string(),
        latest_modified_ns: latest,
        activity_root: scope.project.clone(),
        blocked,
    })
}

fn swiftpm(
    root: &Root,
    artifact: &Path,
    scope: Scope,
    cancel: &AtomicBool,
) -> Result<Option<ProviderEvidence>> {
    let Some(package) = capture(&scope.project, "Package.swift", cancel, &[])? else {
        return Ok(None);
    };
    let mut captures = vec![package];
    if let Some(resolved) = capture(&scope.project, "Package.resolved", cancel, &[])? {
        captures.push(resolved);
    }
    Ok(Some(finish(
        root, artifact, &scope, cancel, "swiftpm", captures, None,
    )?))
}

fn dart(
    root: &Root,
    artifact: &Path,
    scope: Scope,
    cancel: &AtomicBool,
) -> Result<Option<ProviderEvidence>> {
    let Some(pubspec) = capture(&scope.project, "pubspec.yaml", cancel, &[])? else {
        return Ok(None);
    };
    let mut captures = vec![pubspec];
    for name in ["pubspec.lock", "pubspec_overrides.yaml"] {
        if let Some(captured) = capture(&scope.project, name, cancel, &[])? {
            captures.push(captured);
        }
    }
    Ok(Some(finish(
        root, artifact, &scope, cancel, "dart", captures, None,
    )?))
}

fn flutter(
    root: &Root,
    artifact: &Path,
    scope: Scope,
    cancel: &AtomicBool,
) -> Result<Option<ProviderEvidence>> {
    let Some(pubspec) = capture(&scope.project, "pubspec.yaml", cancel, &[])? else {
        return Ok(None);
    };
    let metadata = capture(&scope.project, ".metadata", cancel, &[])?;
    if !pubspec.declares_flutter_sdk && !metadata.as_ref().is_some_and(|m| m.is_flutter_metadata) {
        return Ok(None);
    }
    let mut captures = vec![pubspec];
    if let Some(metadata) = metadata {
        captures.push(metadata);
    }
    for name in ["pubspec.lock", "pubspec_overrides.yaml"] {
        if let Some(captured) = capture(&scope.project, name, cancel, &[])? {
            captures.push(captured);
        }
    }
    Ok(Some(finish(
        root, artifact, &scope, cancel, "flutter", captures, None,
    )?))
}

fn zig(
    root: &Root,
    artifact: &Path,
    scope: Scope,
    cancel: &AtomicBool,
) -> Result<Option<ProviderEvidence>> {
    let Some(build) = capture(&scope.project, "build.zig", cancel, &[])? else {
        return Ok(None);
    };
    let mut captures = vec![build];
    if let Some(zon) = capture(&scope.project, "build.zig.zon", cancel, &[])? {
        captures.push(zon);
    }
    Ok(Some(finish(
        root, artifact, &scope, cancel, "zig", captures, None,
    )?))
}

fn gradle(
    root: &Root,
    artifact: &Path,
    scope: Scope,
    cancel: &AtomicBool,
) -> Result<Option<ProviderEvidence>> {
    const TOKENS: [&[u8]; 2] = [b"buildDir", b"buildDirectory"];
    let names = [
        "settings.gradle",
        "settings.gradle.kts",
        "build.gradle",
        "build.gradle.kts",
    ];
    let captures = capture_any(&scope.project, &names, cancel, &TOKENS)?;
    if captures.is_empty() {
        return Ok(None);
    }
    let blocked = captures.iter().any(|capture| capture.explicit_override).then(|| {
        "Gradle explicitly configures a non-default build directory; output ownership is ambiguous".into()
    });
    Ok(Some(finish(
        root, artifact, &scope, cancel, "gradle", captures, blocked,
    )?))
}

fn dotnet(
    root: &Root,
    artifact: &Path,
    scope: Scope,
    cancel: &AtomicBool,
) -> Result<Option<ProviderEvidence>> {
    const TOKENS: [&[u8]; 8] = [
        b"<Import",
        b"BaseOutputPath",
        b"BaseIntermediateOutputPath",
        b"MSBuildProjectExtensionsPath",
        b"OutputPath",
        b"OutDir",
        b"UseArtifactsOutput",
        b"ArtifactsPath",
    ];
    let project_names = [".csproj", ".fsproj", ".vbproj"];
    let mut names = Vec::new();
    // The extension is matched lexically, but only direct regular files are
    // captured. No recursive search is performed and no project XML is
    // evaluated. A large marker directory is ambiguous and fails closed.
    let mut directory = safety::Directory::open(&scope.project)?;
    for index in 0.. {
        safety::cancelled(cancel)?;
        let Some(entry) = directory.next(cancel)? else {
            break;
        };
        if index >= MAX_DIRECT_MARKERS {
            return Err("Too many direct .NET project markers; ownership is ambiguous".into());
        }
        let Some(name) = entry.path.file_name().and_then(OsStr::to_str) else {
            continue;
        };
        if project_names
            .iter()
            .any(|extension| name.ends_with(extension))
        {
            if names.len() >= MAX_PROVIDER_FILES {
                return Err("Too many direct .NET project markers; ownership is ambiguous".into());
            }
            names.push(name.to_owned());
        }
    }
    names.sort_unstable();
    names.dedup();
    if names.is_empty() {
        return Ok(None);
    }
    let mut captures = capture_any(
        &scope.project,
        &names.iter().map(String::as_str).collect::<Vec<_>>(),
        cancel,
        &TOKENS,
    )?;
    let mut ancestor = Some(scope.project.as_path());
    while let Some(directory) = ancestor {
        for name in ["Directory.Build.props", "Directory.Build.targets"] {
            if present(&directory.join(name))? && captures.len() >= MAX_PROVIDER_FILES {
                return Err("Too many .NET evidence files; ownership is ambiguous".into());
            }
            if let Some(captured) = capture(directory, name, cancel, &TOKENS)? {
                captures.push(captured);
            }
        }
        if directory == root.path {
            break;
        }
        ancestor = directory
            .parent()
            .filter(|parent| parent.starts_with(&root.path));
    }
    let blocked = captures
        .iter()
        .any(|capture| capture.explicit_override)
        .then(|| {
            "MSBuild explicitly configures output paths; bin/obj ownership is ambiguous".into()
        });
    Ok(Some(finish(
        root, artifact, &scope, cancel, "dotnet", captures, blocked,
    )?))
}

/// Identify one exact artifact boundary.  A `build` directory is accepted only
/// when exactly one of Flutter or Gradle supplies direct project evidence.
pub(crate) fn identify(
    root: &Root,
    artifact: &Path,
    cancel: &AtomicBool,
) -> Result<Option<ProviderEvidence>> {
    let Some(scope) = ensure_scope(root, artifact, cancel)? else {
        return Ok(None);
    };
    match artifact.file_name().and_then(OsStr::to_str) {
        Some(".build") => swiftpm(root, artifact, scope, cancel),
        Some(".dart_tool") => dart(root, artifact, scope, cancel),
        Some(".zig-cache") => zig(root, artifact, scope, cancel),
        Some(".gradle") => gradle(root, artifact, scope, cancel),
        Some("bin" | "obj") => dotnet(root, artifact, scope, cancel),
        Some("build") => {
            let flutter = flutter(root, artifact, scope.clone(), cancel)?;
            let gradle = gradle(root, artifact, scope, cancel)?;
            match (flutter, gradle) {
                (Some(_), Some(_)) => Ok(None),
                (Some(found), None) | (None, Some(found)) => Ok(Some(found)),
                (None, None) => Ok(None),
            }
        }
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn root(base: &Path) -> Root {
        safety::authorize(&base.canonicalize().unwrap(), "projects").unwrap()
    }

    #[test]
    fn exact_names_are_only_dispatch() {
        assert!(recognizes_name(OsStr::new(".build")));
        assert!(recognizes_name(OsStr::new("obj")));
        assert!(!recognizes_name(OsStr::new("my-build")));
    }

    #[test]
    fn swiftpm_requires_direct_marker_and_fingerprints_captured_bytes() {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().canonicalize().unwrap();
        let project = base.join("Package");
        let artifact = project.join(".build");
        fs::create_dir_all(&artifact).unwrap();
        let authorized = root(&base);
        assert!(
            identify(&authorized, &artifact, &AtomicBool::new(false))
                .unwrap()
                .is_none()
        );
        fs::write(
            project.join("Package.swift"),
            "// swift-tools-version: 6.0\n",
        )
        .unwrap();
        let first = identify(&authorized, &artifact, &AtomicBool::new(false))
            .unwrap()
            .unwrap();
        assert_eq!(first.kind, "swiftpm");
        assert!(first.blocked.is_none());
        fs::write(project.join("Package.swift"), "// changed\n").unwrap();
        let second = identify(&authorized, &artifact, &AtomicBool::new(false))
            .unwrap()
            .unwrap();
        assert_ne!(first.fingerprint, second.fingerprint);
    }

    #[test]
    fn direct_marker_symlink_is_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().canonicalize().unwrap();
        let project = base.join("Package");
        let artifact = project.join(".build");
        fs::create_dir_all(&artifact).unwrap();
        let outside = base.join("outside.swift");
        fs::write(&outside, "// outside").unwrap();
        std::os::unix::fs::symlink(&outside, project.join("Package.swift")).unwrap();
        let error = identify(&root(&base), &artifact, &AtomicBool::new(false)).unwrap_err();
        assert!(
            error.contains("regular") || error.contains("evidence"),
            "{error}"
        );
    }

    #[test]
    fn dotnet_explicit_output_property_is_blocked() {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().canonicalize().unwrap();
        let project = base.join("Dotnet");
        let artifact = project.join("obj");
        fs::create_dir_all(&artifact).unwrap();
        fs::write(
            project.join("App.csproj"),
            "<Project><PropertyGroup><BaseOutputPath>../out</BaseOutputPath></PropertyGroup></Project>",
        )
        .unwrap();
        let evidence = identify(&root(&base), &artifact, &AtomicBool::new(false))
            .unwrap()
            .unwrap();
        assert_eq!(evidence.kind, "dotnet");
        assert!(evidence.blocked.is_some());
    }

    #[test]
    fn dotnet_inherited_output_config_is_captured_and_blocks() {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().canonicalize().unwrap();
        let project = base.join("nested/Dotnet");
        let artifact = project.join("obj");
        fs::create_dir_all(&artifact).unwrap();
        fs::write(project.join("App.csproj"), "<Project />").unwrap();
        fs::write(
            base.join("Directory.Build.props"),
            "<Project><PropertyGroup><BaseIntermediateOutputPath>../out</BaseIntermediateOutputPath></PropertyGroup></Project>",
        )
        .unwrap();
        let evidence = identify(&root(&base), &artifact, &AtomicBool::new(false))
            .unwrap()
            .unwrap();
        assert_eq!(evidence.kind, "dotnet");
        assert!(evidence.blocked.is_some());
    }

    #[test]
    fn flutter_requires_flutter_manifest_evidence_not_a_folder_name() {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().canonicalize().unwrap();
        let project = base.join("DartWithIos");
        let artifact = project.join("build");
        fs::create_dir_all(project.join("ios")).unwrap();
        fs::create_dir_all(&artifact).unwrap();
        fs::write(project.join("pubspec.yaml"), "name: ordinary_dart\n").unwrap();
        assert!(
            identify(&root(&base), &artifact, &AtomicBool::new(false))
                .unwrap()
                .is_none()
        );
        fs::write(
            project.join(".metadata"),
            "version:\n  revision: abc\nproject_type: app\n",
        )
        .unwrap();
        assert_eq!(
            identify(&root(&base), &artifact, &AtomicBool::new(false))
                .unwrap()
                .unwrap()
                .kind,
            "flutter"
        );
    }

    #[test]
    fn dotnet_marker_enumeration_is_bounded_before_capture() {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().canonicalize().unwrap();
        let project = base.join("ManyMarkers");
        let artifact = project.join("obj");
        fs::create_dir_all(&artifact).unwrap();
        fs::write(project.join("App.csproj"), "<Project />").unwrap();
        for index in 0..=MAX_DIRECT_MARKERS {
            fs::write(project.join(format!("unrelated-{index:03}")), b"x").unwrap();
        }
        let error = identify(&root(&base), &artifact, &AtomicBool::new(false)).unwrap_err();
        assert!(
            error.contains("Too many direct .NET project markers"),
            "{error}"
        );
    }

    #[test]
    fn swiftpm_manifest_text_is_captured_without_execution() {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().canonicalize().unwrap();
        let project = base.join("NoExecution");
        let artifact = project.join(".build");
        fs::create_dir_all(&artifact).unwrap();
        let marker = project.join("side-effect");
        fs::write(
            project.join("Package.swift"),
            format!(
                "// swift-tools-version: 6.0\n// touch {}\n",
                marker.display()
            ),
        )
        .unwrap();
        let evidence = identify(&root(&base), &artifact, &AtomicBool::new(false))
            .unwrap()
            .unwrap();
        assert_eq!(evidence.kind, "swiftpm");
        assert!(!marker.exists());
    }

    #[test]
    fn other_provider_markers_are_direct_and_build_names_are_disambiguated() {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().canonicalize().unwrap();
        let authorized = root(&base);

        let dart_project = base.join("Dart");
        let dart_artifact = dart_project.join(".dart_tool");
        fs::create_dir_all(&dart_artifact).unwrap();
        fs::write(dart_project.join("pubspec.yaml"), "name: fixture\n").unwrap();
        assert_eq!(
            identify(&authorized, &dart_artifact, &AtomicBool::new(false))
                .unwrap()
                .unwrap()
                .kind,
            "dart"
        );

        let flutter_project = base.join("Flutter");
        let flutter_artifact = flutter_project.join("build");
        fs::create_dir_all(flutter_project.join("ios")).unwrap();
        fs::create_dir_all(&flutter_artifact).unwrap();
        fs::write(
            flutter_project.join("pubspec.yaml"),
            "name: fixture\ndependencies:\n  flutter:\n    sdk: flutter\n",
        )
        .unwrap();
        assert_eq!(
            identify(&authorized, &flutter_artifact, &AtomicBool::new(false))
                .unwrap()
                .unwrap()
                .kind,
            "flutter"
        );

        let gradle_project = base.join("Gradle");
        let gradle_artifact = gradle_project.join(".gradle");
        fs::create_dir_all(&gradle_artifact).unwrap();
        fs::write(
            gradle_project.join("settings.gradle"),
            "rootProject.name='fixture'\n",
        )
        .unwrap();
        assert_eq!(
            identify(&authorized, &gradle_artifact, &AtomicBool::new(false))
                .unwrap()
                .unwrap()
                .kind,
            "gradle"
        );

        let zig_project = base.join("Zig");
        let zig_artifact = zig_project.join(".zig-cache");
        fs::create_dir_all(&zig_artifact).unwrap();
        fs::write(
            zig_project.join("build.zig"),
            "pub fn build(_: anytype) void {}\n",
        )
        .unwrap();
        assert_eq!(
            identify(&authorized, &zig_artifact, &AtomicBool::new(false))
                .unwrap()
                .unwrap()
                .kind,
            "zig"
        );

        let ambiguous_project = base.join("Ambiguous");
        let ambiguous_artifact = ambiguous_project.join("build");
        fs::create_dir_all(ambiguous_project.join("ios")).unwrap();
        fs::create_dir_all(&ambiguous_artifact).unwrap();
        fs::write(
            ambiguous_project.join("pubspec.yaml"),
            "name: fixture\ndependencies:\n  flutter:\n    sdk: flutter\n",
        )
        .unwrap();
        fs::write(
            ambiguous_project.join("settings.gradle"),
            "rootProject.name='fixture'\n",
        )
        .unwrap();
        assert!(
            identify(&authorized, &ambiguous_artifact, &AtomicBool::new(false))
                .unwrap()
                .is_none()
        );
    }
}
