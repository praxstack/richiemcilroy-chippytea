//! Read-only review adapters for caches owned by package managers and Docker.
//!
//! Installed-tool review is explicit and uses a write/network-restricted child
//! with bounded output and deadlines. Every command below is fixed in this source file; no manifest,
//! front-end string, or discovered basename can become a command.

use crate::probe;
use serde::Serialize;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

pub const MAX_OUTPUT_BYTES: usize = 64 * 1024;
pub const MAX_OBSERVATIONS: usize = 256;
const MAX_TEXT_BYTES: usize = 512;

pub const HOMEBREW_DOCS: &str = "https://docs.brew.sh/Manpage";
pub const UV_CACHE_DOCS: &str = "https://docs.astral.sh/uv/concepts/cache/";
pub const UV_STORAGE_DOCS: &str = "https://docs.astral.sh/uv/reference/storage/";
pub const PNPM_STORE_DOCS: &str = "https://pnpm.io/cli/store";
pub const DOCKER_DU_DOCS: &str = "https://docs.docker.com/reference/cli/docker/system/df/";
const DOCKER_API_DOCS: &str = "https://docs.docker.com/reference/api/engine/version/v1.46/";
pub const DOCKER_PRUNE_DOCS: &str = "https://docs.docker.com/reference/cli/docker/builder/prune/";
const DOCKER_CONTEXT_DOCS: &str = "https://docs.docker.com/reference/cli/docker/context/inspect/";
const EDITOR_RECORD_DOCS: &str = "https://github.com/microsoft/vscode/blob/main/src/vs/platform/extensionManagement/node/extensionManagementService.ts";
const SANDBOX_EXECUTABLE: &str = "/usr/bin/sandbox-exec";
const FIXED_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin:/opt/homebrew/bin:/usr/local/bin";
const PROBE_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Provider {
    Homebrew,
    Uv,
    Pnpm,
    DockerBuildKit,
    VsCodeExtensions,
    CursorExtensions,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ReviewState {
    Complete,
    Partial,
    Unknown,
}

/// Review output never grants an executable cleanup operation. The owner
/// follow-up is deliberately text, for a separately confirmed user action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum CleanupAuthority {
    ReviewOnly,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReviewEvidence {
    pub source: String,
    pub detail: String,
    pub verified: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReviewObservation {
    pub id: String,
    pub description: String,
    /// Logical bytes represented by an owner record. This may exceed host
    /// reclaimable bytes when records share hardlinks, layers, or clones.
    pub logical_bytes: Option<u64>,
    /// Host bytes are reported only when the owner output proves them. Unknown
    /// is safer than treating logical cache size as physical recovery.
    pub host_bytes: Option<u64>,
    pub reclaimable: Option<bool>,
    pub evidence: Vec<ReviewEvidence>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProviderReview {
    pub provider: Provider,
    pub state: ReviewState,
    pub observations: Vec<ReviewObservation>,
    pub logical_recovery_bytes: u64,
    pub host_recovery_bytes: Option<u64>,
    pub evidence: Vec<ReviewEvidence>,
    pub consequence: String,
    pub owner_followup: String,
    pub cleanup_authority: CleanupAuthority,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Brew,
    Uv,
    Pnpm,
    Docker,
    ScanHelper,
}

impl Tool {
    fn basename(self) -> &'static str {
        match self {
            Self::Brew => "brew",
            Self::Uv => "uv",
            Self::Pnpm => "pnpm",
            Self::Docker => "docker",
            Self::ScanHelper => "chippytea-scan-helper",
        }
    }
}

#[derive(Debug, Clone)]
pub struct CommandSpec {
    pub tool: Tool,
    pub path: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub timeout_ms: u64,
    pub output_cap_bytes: usize,
}

impl CommandSpec {
    fn fixed(tool: Tool, path: &Path, args: &[&str], env: &[(&str, &str)]) -> Result<Self, String> {
        validate_executable(tool, path)?;
        let args = args.iter().map(|arg| (*arg).to_owned()).collect();
        let env = env
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect();
        Ok(Self {
            tool,
            path: path.to_owned(),
            args,
            env,
            timeout_ms: 15_000,
            output_cap_bytes: MAX_OUTPUT_BYTES,
        })
    }

    pub fn display(&self) -> String {
        let mut result = self.path.to_string_lossy().into_owned();
        for arg in &self.args {
            result.push(' ');
            result.push_str(arg);
        }
        bound_text(&result)
    }
}

#[derive(Debug, Clone)]
pub struct CommandOutput {
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
    pub truncated: bool,
}

impl CommandOutput {
    fn succeeded(&self) -> bool {
        self.exit_code == Some(0) && !self.timed_out
    }

    fn text(&self) -> String {
        let mut text = self.stdout.clone();
        if !self.stderr.trim().is_empty() {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&self.stderr);
        }
        bound_prefix(&text, MAX_OUTPUT_BYTES).0.to_owned()
    }
}

pub trait CommandRunner {
    fn run(&self, command: &CommandSpec) -> Result<CommandOutput, String>;
}

/// Run one explicitly confirmed, read-only owner inspection. This is the only
/// production entry point: executable resolution is fixed and the subprocess
/// is always placed behind the write/network-denying sandbox.
pub(crate) fn review_installed(
    provider: &str,
    cancel: &AtomicBool,
) -> Result<ProviderReview, String> {
    let provider = match provider {
        "homebrew" => Provider::Homebrew,
        "uv" => Provider::Uv,
        "pnpm" => Provider::Pnpm,
        "docker" => Provider::DockerBuildKit,
        "vscode_extensions" => Provider::VsCodeExtensions,
        "cursor_extensions" => Provider::CursorExtensions,
        _ => return Err("unknown installed provider".into()),
    };
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or("HOME is unavailable for installed-tool review")?;
    let tool = match provider {
        Provider::Homebrew => Tool::Brew,
        Provider::Uv => Tool::Uv,
        Provider::Pnpm => Tool::Pnpm,
        Provider::DockerBuildKit => Tool::Docker,
        Provider::VsCodeExtensions | Provider::CursorExtensions => Tool::ScanHelper,
    };
    let Some(path) = resolve_installed(tool, &home) else {
        return Ok(unknown_review(
            provider,
            format!(
                "No trusted installed {} executable was found",
                tool.basename()
            ),
            docs_for(provider),
            tool.basename(),
        ));
    };
    let config = match storage_environment(tool, |key| std::env::var_os(key)) {
        Ok(config) => config,
        Err(error) => {
            return Ok(unknown_review(
                provider,
                error,
                docs_for(provider),
                tool.basename(),
            ));
        }
    };
    let runner = InstalledRunner {
        cancel,
        home,
        config,
    };
    let mut review = match provider {
        Provider::Homebrew => review_homebrew(&runner, &path),
        Provider::Uv => review_uv(&runner, &path),
        Provider::Pnpm => review_pnpm(&runner, &path),
        Provider::DockerBuildKit => review_docker_installed(&runner, &path, &runner.home, cancel),
        Provider::VsCodeExtensions | Provider::CursorExtensions => {
            review_editor(&runner, &path, &runner.home, provider)
        }
    };
    review.evidence.push(ReviewEvidence {
        source: "configuration-scope".into(),
        detail: "User-level configuration and validated storage settings visible to chippytea; no project configuration or shell profiles are loaded. Docker uses its default local context only.".into(),
        verified: true,
    });
    Ok(review)
}

fn docs_for(provider: Provider) -> &'static str {
    match provider {
        Provider::Homebrew => HOMEBREW_DOCS,
        Provider::Uv => UV_CACHE_DOCS,
        Provider::Pnpm => PNPM_STORE_DOCS,
        Provider::DockerBuildKit => DOCKER_DU_DOCS,
        Provider::VsCodeExtensions | Provider::CursorExtensions => EDITOR_RECORD_DOCS,
    }
}

fn resolve_installed(tool: Tool, home: &Path) -> Option<PathBuf> {
    if tool == Tool::ScanHelper {
        #[cfg(not(test))]
        {
            return crate::scan_worker::bundled_helper();
        }
        #[cfg(test)]
        {
            return None;
        }
    }
    let candidates: Vec<(PathBuf, PathBuf)> = match tool {
        Tool::Brew => vec![
            (
                PathBuf::from("/opt/homebrew/bin/brew"),
                PathBuf::from("/opt/homebrew"),
            ),
            (
                PathBuf::from("/usr/local/bin/brew"),
                PathBuf::from("/usr/local"),
            ),
        ],
        Tool::Uv => vec![
            (home.join(".local/bin/uv"), home.join(".local")),
            (
                PathBuf::from("/opt/homebrew/bin/uv"),
                PathBuf::from("/opt/homebrew"),
            ),
            (
                PathBuf::from("/usr/local/bin/uv"),
                PathBuf::from("/usr/local"),
            ),
        ],
        Tool::Pnpm => vec![
            (home.join("Library/pnpm/pnpm"), home.join("Library/pnpm")),
            (
                home.join(".local/share/pnpm/pnpm"),
                home.join(".local/share/pnpm"),
            ),
            (
                PathBuf::from("/opt/homebrew/bin/pnpm"),
                PathBuf::from("/opt/homebrew"),
            ),
            (
                PathBuf::from("/usr/local/bin/pnpm"),
                PathBuf::from("/usr/local"),
            ),
        ],
        Tool::Docker => vec![
            (
                PathBuf::from("/Applications/Docker.app/Contents/Resources/bin/docker"),
                PathBuf::from("/Applications/Docker.app/Contents/Resources"),
            ),
            (
                PathBuf::from("/opt/homebrew/bin/docker"),
                PathBuf::from("/opt/homebrew"),
            ),
            (
                PathBuf::from("/usr/local/bin/docker"),
                PathBuf::from("/usr/local"),
            ),
        ],
        Tool::ScanHelper => unreachable!("the bundled helper is resolved before installed tools"),
    };
    candidates
        .into_iter()
        .find_map(|(candidate, prefix)| resolve_candidate(tool, &candidate, &prefix))
}

fn resolve_candidate(tool: Tool, candidate: &Path, prefix: &Path) -> Option<PathBuf> {
    use std::os::unix::fs::MetadataExt;
    let canonical = std::fs::canonicalize(candidate).ok()?;
    let prefix = std::fs::canonicalize(prefix).ok()?;
    let metadata = std::fs::metadata(&canonical).ok()?;
    // pnpm's documented npm/Homebrew install is a symlink to bin/pnpm.cjs.
    // Permit that exact installed entry point, never an arbitrary script name.
    (canonical.starts_with(&prefix)
        && validate_executable(tool, &canonical).is_ok()
        && metadata.is_file()
        && metadata.mode() & 0o111 != 0
        && metadata.mode() & 0o022 == 0
        && (metadata.uid() == 0 || metadata.uid() == unsafe { libc::geteuid() }))
    .then_some(canonical)
}

/// Preserve only storage-related settings, never PATH, loaders, credentials,
/// shell initialization, plugins, or Docker remote-host overrides. GUI apps do
/// not inherit every terminal setting: the result explicitly states its scope.
fn storage_environment(
    tool: Tool,
    lookup: impl Fn(&str) -> Option<std::ffi::OsString>,
) -> Result<Vec<(String, String)>, String> {
    let keys: &[&str] = match tool {
        Tool::Brew => &[
            "HOMEBREW_CACHE",
            "HOMEBREW_LOGS",
            "HOMEBREW_TEMP",
            "HOMEBREW_CLEANUP_MAX_AGE_DAYS",
            "HOMEBREW_NO_CLEANUP_FORMULAE",
        ],
        Tool::Uv => &[
            "UV_CACHE_DIR",
            "UV_CONFIG_FILE",
            "XDG_CACHE_HOME",
            "XDG_CONFIG_HOME",
        ],
        Tool::Pnpm => &[
            "PNPM_HOME",
            "XDG_CACHE_HOME",
            "XDG_CONFIG_HOME",
            "XDG_DATA_HOME",
            "npm_config_store_dir",
            "NPM_CONFIG_STORE_DIR",
            "npm_config_userconfig",
            "NPM_CONFIG_USERCONFIG",
        ],
        Tool::Docker | Tool::ScanHelper => &[],
    };
    let mut values = Vec::new();
    for &key in keys {
        let Some(value) = lookup(key) else { continue };
        let value = value
            .into_string()
            .map_err(|_| format!("{key} is not a supported UTF-8 storage setting"))?;
        if value.is_empty() || value.len() > MAX_TEXT_BYTES || value.chars().any(char::is_control) {
            return Err(format!("{key} is not a supported bounded storage setting"));
        }
        let valid = match key {
            "HOMEBREW_CLEANUP_MAX_AGE_DAYS" => {
                value.bytes().all(|byte| byte.is_ascii_digit()) && value.parse::<u32>().is_ok()
            }
            "HOMEBREW_NO_CLEANUP_FORMULAE" => value.split(',').all(|name| {
                !name.is_empty()
                    && name
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"/+@._-".contains(&byte))
            }),
            _ => {
                Path::new(&value).is_absolute()
                    && !Path::new(&value)
                        .components()
                        .any(|part| matches!(part, Component::CurDir | Component::ParentDir))
            }
        };
        if !valid {
            return Err(format!(
                "{key} could not be preserved safely; no default-cache result is substituted"
            ));
        }
        values.push((key.to_owned(), value));
    }
    for (lower, upper) in [
        ("npm_config_store_dir", "NPM_CONFIG_STORE_DIR"),
        ("npm_config_userconfig", "NPM_CONFIG_USERCONFIG"),
    ] {
        let lower = values.iter().find(|(key, _)| key == lower);
        let upper = values.iter().find(|(key, _)| key == upper);
        if let (Some((_, lower)), Some((_, upper))) = (lower, upper)
            && lower != upper
        {
            return Err(
                "Conflicting pnpm storage settings; no default-store result is substituted".into(),
            );
        }
    }
    Ok(values)
}

struct InstalledRunner<'a> {
    cancel: &'a AtomicBool,
    home: PathBuf,
    config: Vec<(String, String)>,
}

impl CommandRunner for InstalledRunner<'_> {
    fn run(&self, command: &CommandSpec) -> Result<CommandOutput, String> {
        if self.cancel.load(std::sync::atomic::Ordering::Acquire) {
            return Err("installed-tool review cancelled".into());
        }
        let mut sandboxed = Command::new(SANDBOX_EXECUTABLE);
        sandboxed
            .arg("-p")
            .arg(sandbox_profile())
            .current_dir("/")
            .env_clear()
            .env("PATH", FIXED_PATH)
            .env("HOME", &self.home)
            .envs(self.config.iter().map(|(key, value)| (key, value)))
            .arg(&command.path)
            .args(&command.args);
        for (key, value) in &command.env {
            if matches!(
                key.as_str(),
                "HOMEBREW_NO_AUTO_UPDATE"
                    | "HOMEBREW_NO_AUTOREMOVE"
                    | "HOMEBREW_NO_INSTALL_CLEANUP"
                    | "HOMEBREW_NO_ANALYTICS"
            ) {
                sandboxed.env(key, value);
            }
        }
        let timeout = Duration::from_millis(command.timeout_ms).min(PROBE_TIMEOUT);
        let output_cap = command.output_cap_bytes.min(MAX_OUTPUT_BYTES);
        let _tool = command.tool;
        let output = probe::run(sandboxed, timeout, output_cap, Some(self.cancel))?;
        Ok(CommandOutput {
            exit_code: output.status.code(),
            stdout: String::from_utf8(output.stdout)
                .map_err(|_| "installed-tool output was not UTF-8".to_owned())?,
            stderr: String::from_utf8(output.stderr)
                .map_err(|_| "installed-tool diagnostics were not UTF-8".to_owned())?,
            timed_out: false,
            truncated: false,
        })
    }
}

fn sandbox_profile() -> &'static str {
    // Modern macOS loads executables through system-managed Cryptex paths;
    // a guessed /usr + /System read allowlist can abort even /usr/bin/true.
    // Inspection may read the user's configuration, but cannot write files,
    // contact any socket (including Docker), or delegate via default-denied IPC.
    // Docker accounting instead uses our fixed-method, fixed-route Unix client.
    "(version 1)\n\
         (deny default)\n\
         (allow process-exec)\n\
         (allow process-fork)\n\
         (allow sysctl-read)\n\
         (allow file-read*)\n\
         (allow file-map-executable)\n\
         (deny file-write*)\n\
         (deny network*)"
}

pub fn review_homebrew<R: CommandRunner>(runner: &R, path: &Path) -> ProviderReview {
    let command = match CommandSpec::fixed(
        Tool::Brew,
        path,
        &["cleanup", "--dry-run", "--verbose"],
        &[
            ("HOMEBREW_NO_AUTO_UPDATE", "1"),
            ("HOMEBREW_NO_AUTOREMOVE", "1"),
            ("HOMEBREW_NO_INSTALL_CLEANUP", "1"),
            ("HOMEBREW_NO_ANALYTICS", "1"),
        ],
    ) {
        Ok(command) => command,
        Err(error) => {
            return unknown_review(
                Provider::Homebrew,
                error,
                HOMEBREW_DOCS,
                "brew cleanup --dry-run --verbose",
            );
        }
    };
    let display = command.display();
    let output = match runner.run(&command) {
        Ok(output) => output,
        Err(error) => return unknown_review(Provider::Homebrew, error, HOMEBREW_DOCS, &display),
    };
    let command_evidence = command_evidence(&display, &output);
    if !output.succeeded() {
        return review_with_state(
            Provider::Homebrew,
            ReviewState::Unknown,
            Vec::new(),
            vec![command_evidence, docs_evidence(HOMEBREW_DOCS)],
            "Homebrew did not provide a trustworthy dry-run result; no cache recovery is claimed.",
            "After reviewing Homebrew's own output, a user may run `brew cleanup` if desired.",
        );
    };
    let (observations, logical_bytes, partial) = parse_homebrew(&output.text(), HOMEBREW_DOCS);
    review_with_state(
        Provider::Homebrew,
        if partial || output.truncated || output.stdout.len() > MAX_OUTPUT_BYTES {
            ReviewState::Partial
        } else {
            ReviewState::Complete
        },
        observations,
        vec![command_evidence, docs_evidence(HOMEBREW_DOCS)],
        "Homebrew's dry-run describes owner-selected stale downloads and old versions; it does not prove physical bytes reclaimable on this host.",
        "Review the dry-run, then use Homebrew's owner-native `brew cleanup` command only after explicit confirmation.",
    )
    .with_logical_bytes(logical_bytes)
}

pub fn review_uv<R: CommandRunner>(runner: &R, path: &Path) -> ProviderReview {
    let command = match CommandSpec::fixed(Tool::Uv, path, &["cache", "dir"], &[]) {
        Ok(command) => command,
        Err(error) => return unknown_review(Provider::Uv, error, UV_CACHE_DOCS, "uv cache dir"),
    };
    let display = command.display();
    let output = match runner.run(&command) {
        Ok(output) => output,
        Err(error) => return unknown_review(Provider::Uv, error, UV_CACHE_DOCS, &display),
    };
    let command_evidence = command_evidence(&display, &output);
    let Some(cache_dir) = parse_single_absolute_path(&output) else {
        return review_with_state(
            Provider::Uv,
            ReviewState::Unknown,
            Vec::new(),
            vec![
                command_evidence,
                docs_evidence(UV_CACHE_DOCS),
                docs_evidence(UV_STORAGE_DOCS),
            ],
            "uv did not identify its active cache directory; cache ownership and recovery are unknown.",
            "Use uv's documented owner-native cache commands after separately reviewing the cache directory.",
        );
    };
    let cache_dir = bound_text(cache_dir);
    let observation = ReviewObservation {
        id: "uv-cache".into(),
        description: format!("Active uv cache directory: {cache_dir}"),
        logical_bytes: None,
        host_bytes: None,
        reclaimable: None,
        evidence: vec![command_evidence.clone(), docs_evidence(UV_STORAGE_DOCS)],
    };
    review_with_state(
        Provider::Uv,
        ReviewState::Partial,
        vec![observation],
        vec![
            command_evidence,
            docs_evidence(UV_CACHE_DOCS),
            docs_evidence(UV_STORAGE_DOCS),
        ],
        "uv identified a disposable cache directory, but this read-only adapter does not infer its size or which entries are unused.",
        "Review the path, then use uv's owner-native `uv cache prune` or `uv cache clean` only after explicit confirmation; no dry-run prune flag is assumed.",
    )
}

pub fn review_pnpm<R: CommandRunner>(runner: &R, path: &Path) -> ProviderReview {
    let path_command = match CommandSpec::fixed(Tool::Pnpm, path, &["store", "path"], &[]) {
        Ok(command) => command,
        Err(error) => {
            return unknown_review(Provider::Pnpm, error, PNPM_STORE_DOCS, "pnpm store path");
        }
    };
    let path_display = path_command.display();
    let path_output = match runner.run(&path_command) {
        Ok(output) => output,
        Err(error) => return unknown_review(Provider::Pnpm, error, PNPM_STORE_DOCS, &path_display),
    };
    let path_evidence = command_evidence(&path_display, &path_output);
    let Some(store_path) = parse_single_absolute_path(&path_output) else {
        return review_with_state(
            Provider::Pnpm,
            ReviewState::Unknown,
            Vec::new(),
            vec![path_evidence, docs_evidence(PNPM_STORE_DOCS)],
            "pnpm did not identify its active store; ownership and recovery are unknown.",
            "Review the store path, then use pnpm's owner-native store commands only after explicit confirmation.",
        );
    };
    let status_command = match CommandSpec::fixed(Tool::Pnpm, path, &["store", "status"], &[]) {
        Ok(command) => command,
        Err(error) => {
            return unknown_review(Provider::Pnpm, error, PNPM_STORE_DOCS, "pnpm store status");
        }
    };
    let status_display = status_command.display();
    let status_output = match runner.run(&status_command) {
        Ok(output) => output,
        Err(error) => {
            return review_with_state(
                Provider::Pnpm,
                ReviewState::Partial,
                vec![store_path_observation(
                    "pnpm",
                    store_path,
                    &path_evidence,
                    PNPM_STORE_DOCS,
                )],
                vec![
                    path_evidence,
                    docs_evidence(PNPM_STORE_DOCS),
                    ReviewEvidence {
                        source: "runner".into(),
                        detail: bound_text(&error),
                        verified: false,
                    },
                ],
                "pnpm identified its store, but store integrity status could not be checked; no recovery bytes are claimed.",
                "Review the store, then use pnpm's owner-native `pnpm store prune` only after explicit confirmation.",
            );
        }
    };
    let status_evidence = command_evidence(&status_display, &status_output);
    let mut description = format!("Active pnpm store: {}", bound_text(store_path));
    if !status_output.stdout.trim().is_empty() || !status_output.stderr.trim().is_empty() {
        description.push_str("; store status output was recorded");
    }
    let state = if !status_output.succeeded() || status_output.truncated || path_output.truncated {
        ReviewState::Partial
    } else {
        ReviewState::Complete
    };
    review_with_state(
        Provider::Pnpm,
        state,
        vec![ReviewObservation {
            id: "pnpm-store".into(),
            description,
            logical_bytes: None,
            host_bytes: None,
            reclaimable: None,
            evidence: vec![
                path_evidence.clone(),
                status_evidence.clone(),
                docs_evidence(PNPM_STORE_DOCS),
            ],
        }],
        vec![
            path_evidence,
            status_evidence,
            docs_evidence(PNPM_STORE_DOCS),
        ],
        "pnpm identified its owner store and integrity status, but this review does not estimate physical recovery bytes.",
        "Review the status, then use pnpm's owner-native `pnpm store prune` only after explicit confirmation; no dry-run prune flag is assumed.",
    )
}

fn review_editor<R: CommandRunner>(
    runner: &R,
    path: &Path,
    home: &Path,
    provider: Provider,
) -> ProviderReview {
    let (name, editor, directory) = match provider {
        Provider::VsCodeExtensions => ("vscode", crate::editor_review::Editor::Vscode, ".vscode"),
        Provider::CursorExtensions => ("cursor", crate::editor_review::Editor::Cursor, ".cursor"),
        _ => {
            return unknown_review(
                provider,
                "Not an editor metadata provider".into(),
                EDITOR_RECORD_DOCS,
                "editor metadata review",
            );
        }
    };
    let command = match CommandSpec::fixed(Tool::ScanHelper, path, &["--editor-review", name], &[])
    {
        Ok(command) => command,
        Err(error) => {
            return unknown_review(
                provider,
                error,
                EDITOR_RECORD_DOCS,
                "editor metadata review",
            );
        }
    };
    let output = match runner.run(&command) {
        Ok(output) if output.succeeded() && !output.truncated => output,
        Ok(_) => {
            return unknown_review(
                provider,
                "Editor metadata could not be inspected within its read-only bounds".into(),
                EDITOR_RECORD_DOCS,
                &command.display(),
            );
        }
        Err(error) => {
            return unknown_review(provider, error, EDITOR_RECORD_DOCS, &command.display());
        }
    };
    let expected_root = home.join(directory).join("extensions");
    if output.stdout.len() > MAX_OUTPUT_BYTES {
        return unknown_review(
            provider,
            "Editor metadata exceeded its response limit".into(),
            EDITOR_RECORD_DOCS,
            &command.display(),
        );
    }
    let report = match serde_json::from_str::<crate::editor_review::EditorReview>(&output.stdout) {
        Ok(report) if valid_editor_report(&report, editor, &expected_root) => report,
        _ => {
            return unknown_review(
                provider,
                "Editor metadata returned an invalid or unbounded report".into(),
                EDITOR_RECORD_DOCS,
                &command.display(),
            );
        }
    };
    let mut evidence = vec![
        ReviewEvidence {
            source: "editor-default-extension-metadata".into(),
            detail: bound_text(&format!(
                "{} of {} true obsolete markers matched package metadata under {}",
                report.verified_count, report.marker_count, report.extensions_root
            )),
            verified: report.complete,
        },
        docs_evidence(EDITOR_RECORD_DOCS),
    ];
    if let Some(reason) = report.reason {
        evidence.push(ReviewEvidence {
            source: "incomplete-editor-metadata".into(),
            detail: reason,
            verified: false,
        });
    }
    let state = if !report.complete && report.records.is_empty() {
        ReviewState::Unknown
    } else {
        ReviewState::Partial
    };
    let observations = report
        .records
        .into_iter()
        .map(|record| ReviewObservation {
            id: record.marker,
            description: bound_text(&format!(
                "{} {}: exact obsolete marker and package identity match at {}",
                record.extension_id, record.version, record.directory
            )),
            logical_bytes: None,
            host_bytes: None,
            reclaimable: None,
            evidence: vec![docs_evidence(EDITOR_RECORD_DOCS)],
        })
        .collect();
    review_with_state(
        provider,
        state,
        observations,
        evidence,
        "This inspects only default-folder obsolete markers and package metadata. It does not establish an installed editor's identity, active or rollback versions, running extension hosts, custom extension locations, size, or safe removal. No data is changed.",
        "Open the editor's Extensions view to review these records and rollback needs. Obsolete metadata alone does not authorize removing an extension or awarding chips.",
    )
}

fn valid_editor_report(
    report: &crate::editor_review::EditorReview,
    editor: crate::editor_review::Editor,
    expected_root: &Path,
) -> bool {
    let bounded =
        |value: &str| value.len() <= MAX_TEXT_BYTES && !value.chars().any(char::is_control);
    let mut seen = std::collections::HashSet::new();
    report.editor == editor
        && Path::new(&report.extensions_root) == expected_root
        && bounded(&report.extensions_root)
        && report.records.len() <= 64
        && report.marker_count <= 256
        && report.verified_count == report.records.len() as u64
        && report.verified_count <= report.marker_count
        && report.reason.as_deref().is_none_or(bounded)
        && (report.complete || report.reason.is_some())
        && report.records.iter().all(|record| {
            let directory = Path::new(&record.directory);
            record.review_only
                && seen.insert(&record.marker)
                && [
                    record.directory.as_str(),
                    &record.extension_id,
                    &record.version,
                    &record.marker,
                ]
                .into_iter()
                .all(|text| !text.is_empty() && bounded(text))
                && directory.parent() == Some(expected_root)
                && directory.file_name().and_then(|name| name.to_str())
                    == Some(record.marker.as_str())
                && !directory
                    .components()
                    .any(|part| matches!(part, Component::CurDir | Component::ParentDir))
        })
}

fn review_docker_installed<R: CommandRunner>(
    runner: &R,
    path: &Path,
    home: &Path,
    cancel: &AtomicBool,
) -> ProviderReview {
    let context_command = match CommandSpec::fixed(
        Tool::Docker,
        path,
        &[
            "context",
            "inspect",
            "default",
            "--format",
            "{{json .Endpoints.docker}}",
        ],
        &[],
    ) {
        Ok(command) => command,
        Err(error) => {
            return unknown_review(
                Provider::DockerBuildKit,
                error,
                DOCKER_CONTEXT_DOCS,
                "docker context inspect default",
            );
        }
    };
    let context_display = context_command.display();
    let context_output = match runner.run(&context_command) {
        Ok(output) => output,
        Err(error) => {
            return unknown_review(
                Provider::DockerBuildKit,
                error,
                DOCKER_CONTEXT_DOCS,
                &context_display,
            );
        }
    };
    if !context_output.succeeded() || context_output.truncated {
        return unknown_review(
            Provider::DockerBuildKit,
            "Docker context inspection failed".into(),
            DOCKER_CONTEXT_DOCS,
            &context_display,
        );
    }
    let Some(endpoint) = parse_docker_endpoint(&context_output.stdout) else {
        return unknown_review(
            Provider::DockerBuildKit,
            "Docker context returned a missing, malformed, or non-Unix endpoint".into(),
            DOCKER_CONTEXT_DOCS,
            &context_display,
        );
    };
    let Some(socket) = endpoint.strip_prefix("unix://") else {
        return unknown_review(
            Provider::DockerBuildKit,
            "Docker remote endpoints are not permitted".into(),
            DOCKER_CONTEXT_DOCS,
            &context_display,
        );
    };
    let socket = Path::new(socket);
    let known_sockets = [
        home.join(".docker/run/docker.sock"),
        PathBuf::from("/var/run/docker.sock"),
    ];
    let verified_socket = known_sockets
        .iter()
        .any(|known| known == socket)
        .then(|| verify_owned_socket(socket))
        .flatten();
    let Some(socket) = verified_socket else {
        return unknown_review(
            Provider::DockerBuildKit,
            "Docker endpoint is not a known, owned local Unix socket".into(),
            DOCKER_CONTEXT_DOCS,
            &context_display,
        );
    };
    // The installed CLI never receives daemon socket authority. This owned
    // client has no request-method, route, TCP, redirect, or plugin parameter.
    let cache = match crate::docker_read::build_cache(&socket, cancel) {
        Ok(cache) => cache,
        Err(error) => {
            return unknown_review(
                Provider::DockerBuildKit,
                error,
                DOCKER_API_DOCS,
                "GET /v1.46/system/df?type=build-cache (verified local Unix socket)",
            );
        }
    };
    let context_evidence = ReviewEvidence {
        source: "docker-default-local-context".into(),
        detail: bound_text(&format!(
            "Default context; pinned local socket {}",
            socket.display()
        )),
        verified: true,
    };
    let observations = cache
        .records
        .iter()
        .map(|record| ReviewObservation {
            id: record.id.clone(),
            description: bound_text(&format!(
                "{}: {}; in use: {}; shared: {}; owner usage count: {}",
                record.kind, record.description, record.in_use, record.shared, record.usage_count,
            )),
            logical_bytes: Some(record.size),
            host_bytes: None,
            // Not-in-use is not an approved prune plan. Shared layers, retention
            // policy, and later reuse can all prevent those bytes being reclaimed.
            reclaimable: record.in_use.then_some(false),
            evidence: vec![docs_evidence(DOCKER_API_DOCS)],
        })
        .collect();
    let consequence = format!(
        "Docker reports {} logical build-cache bytes, including {} not currently in use. Neither is a prune estimate: shared records and retention policy can prevent recovery, and host APFS recovery from the VM is unknown. Cancelling this review closes our request; daemon-side work may continue.",
        cache.logical_bytes, cache.unused_logical_bytes,
    );
    review_with_state(
        Provider::DockerBuildKit,
        ReviewState::Complete,
        observations,
        vec![context_evidence, docs_evidence(DOCKER_API_DOCS), docs_evidence(DOCKER_PRUNE_DOCS)],
        &consequence,
        "Review Docker's owner-native builder controls before any separately confirmed prune. Images, volumes, containers and system-wide pruning are not inspected or authorized.",
    ).with_logical_bytes(cache.logical_bytes)
}

fn parse_docker_endpoint(text: &str) -> Option<String> {
    if text.len() > MAX_OUTPUT_BYTES || text.lines().count() != 1 {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(text.trim()).ok()?;
    let endpoint = value.get("Host")?.as_str()?.trim();
    value.get("SkipTLSVerify")?.as_bool()?;
    (!endpoint.is_empty()
        && endpoint.len() <= MAX_TEXT_BYTES
        && !endpoint.chars().any(char::is_control))
    .then(|| endpoint.to_owned())
}

#[cfg(unix)]
fn verify_owned_socket(path: &Path) -> Option<PathBuf> {
    use std::os::unix::fs::FileTypeExt;
    use std::os::unix::fs::MetadataExt;
    let canonical = std::fs::canonicalize(path).ok()?;
    let metadata = std::fs::symlink_metadata(&canonical).ok()?;
    (metadata.file_type().is_socket() && metadata.uid() == unsafe { libc::geteuid() })
        .then_some(canonical)
}

#[cfg(not(unix))]
fn verify_owned_socket(_path: &Path) -> Option<PathBuf> {
    None
}

fn validate_executable(tool: Tool, path: &Path) -> Result<(), String> {
    let name = path.file_name().and_then(|name| name.to_str());
    let allowed_name = name == Some(tool.basename())
        || (tool == Tool::Pnpm
            && name == Some("pnpm.cjs")
            && path
                .parent()
                .and_then(Path::file_name)
                .is_some_and(|name| name == "bin")
            && path
                .parent()
                .and_then(Path::parent)
                .and_then(Path::file_name)
                .is_some_and(|name| name == "pnpm"));
    if !path.is_absolute() || !allowed_name {
        return Err(format!(
            "{} must be an absolute allowlisted executable path",
            tool.basename()
        ));
    }
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir | Component::CurDir))
    {
        return Err("executable path contains a relative component".into());
    }
    Ok(())
}

fn review_with_state(
    provider: Provider,
    state: ReviewState,
    observations: Vec<ReviewObservation>,
    evidence: Vec<ReviewEvidence>,
    consequence: &str,
    owner_followup: &str,
) -> ProviderReview {
    ProviderReview {
        provider,
        state,
        observations: observations.into_iter().take(MAX_OBSERVATIONS).collect(),
        logical_recovery_bytes: 0,
        host_recovery_bytes: None,
        evidence,
        consequence: bound_text(consequence),
        owner_followup: bound_text(owner_followup),
        cleanup_authority: CleanupAuthority::ReviewOnly,
    }
}

trait ReviewBuilder {
    fn with_logical_bytes(self, bytes: u64) -> Self;
}

impl ReviewBuilder for ProviderReview {
    fn with_logical_bytes(mut self, bytes: u64) -> Self {
        self.logical_recovery_bytes = bytes;
        self
    }
}

fn unknown_review(provider: Provider, error: String, docs: &str, command: &str) -> ProviderReview {
    review_with_state(
        provider,
        ReviewState::Unknown,
        Vec::new(),
        vec![
            ReviewEvidence {
                source: "runner".into(),
                detail: bound_text(&error),
                verified: false,
            },
            ReviewEvidence {
                source: "command".into(),
                detail: bound_text(command),
                verified: false,
            },
            docs_evidence(docs),
        ],
        "Owner evidence is unavailable or unsafe to interpret; no recovery is claimed.",
        "Use the provider's own documented inspection and cleanup workflow after explicit confirmation.",
    )
}

fn docs_evidence(url: &str) -> ReviewEvidence {
    ReviewEvidence {
        source: "official-documentation".into(),
        detail: url.into(),
        verified: true,
    }
}

fn command_evidence(command: &str, output: &CommandOutput) -> ReviewEvidence {
    ReviewEvidence {
        source: "owner-command".into(),
        detail: format!(
            "{} (exit={:?}, timeout={}, truncated={})",
            bound_text(command),
            output.exit_code,
            output.timed_out,
            output.truncated
        ),
        verified: output.succeeded() && !output.truncated,
    }
}

fn store_path_observation(
    provider: &str,
    path: &str,
    command: &ReviewEvidence,
    docs: &str,
) -> ReviewObservation {
    ReviewObservation {
        id: format!("{provider}-store"),
        description: format!("Active {provider} store: {}", bound_text(path)),
        logical_bytes: None,
        host_bytes: None,
        reclaimable: None,
        evidence: vec![command.clone(), docs_evidence(docs)],
    }
}

fn parse_homebrew(text: &str, docs: &str) -> (Vec<ReviewObservation>, u64, bool) {
    let mut observations = Vec::new();
    let mut total = 0u64;
    let mut partial = false;
    for (index, line) in text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .enumerate()
    {
        if index >= MAX_OBSERVATIONS {
            partial = true;
            break;
        }
        let description = bound_text(line.trim());
        // Homebrew also prints warnings, headings and an aggregate summary.
        // Only a specific dry-run removal line is an owner-selected artifact;
        // do not count size-looking text in filenames, warnings or totals twice.
        let selected = line.trim().strip_prefix("Would remove: ");
        let bytes = selected.and_then(|line| {
            let (_, suffix) = line.rsplit_once(" (")?;
            let suffix = suffix.strip_suffix(')')?.trim();
            let size = if let Some((count, size)) = suffix.rsplit_once(',') {
                let count = count
                    .trim()
                    .strip_suffix(" files")
                    .or_else(|| count.trim().strip_suffix(" file"))?;
                if count.is_empty()
                    || !count
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || byte == b',')
                {
                    return None;
                }
                size.trim()
            } else {
                suffix
            };
            parse_size_token(size)
        });
        if selected.is_some() && bytes.is_none() {
            partial = true;
        }
        if let Some(bytes) = bytes {
            match total.checked_add(bytes) {
                Some(sum) => total = sum,
                None => partial = true,
            }
        }
        observations.push(ReviewObservation {
            id: format!("brew-{index}"),
            description,
            logical_bytes: bytes,
            host_bytes: None,
            reclaimable: selected.map(|_| true),
            evidence: vec![docs_evidence(docs)],
        });
    }
    (observations, total, partial)
}

fn parse_single_absolute_path(output: &CommandOutput) -> Option<&str> {
    if !output.succeeded() || output.truncated || output.stdout.len() > MAX_TEXT_BYTES {
        return None;
    }
    let path = output.stdout.trim();
    if path.is_empty()
        || path.lines().count() != 1
        || path.as_bytes().contains(&0)
        || !Path::new(path).is_absolute()
        || Path::new(path)
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        return None;
    }
    Some(path)
}

fn parse_size_token(token: &str) -> Option<u64> {
    let token = token.trim_matches('*');
    let split_at = token.find(|character: char| character.is_ascii_alphabetic())?;
    let (number, unit) = token.split_at(split_at);
    let multiplier = match unit.to_ascii_lowercase().as_str() {
        "b" => 1,
        "kb" => 1_000,
        "kib" => 1 << 10,
        "mb" => 1_000_000,
        "mib" => 1 << 20,
        "gb" => 1_000_000_000,
        "gib" => 1 << 30,
        "tb" => 1_000_000_000_000,
        "tib" => 1 << 40,
        _ => return None,
    };
    let mut pieces = number.split('.');
    let whole: u64 = pieces.next()?.parse().ok()?;
    let fraction = pieces.next().unwrap_or("");
    if pieces.next().is_some()
        || fraction.len() > 6
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let scale = 10u64.checked_pow(fraction.len() as u32)?;
    let fractional: u64 = if fraction.is_empty() {
        0
    } else {
        fraction.parse().ok()?
    };
    whole
        .checked_mul(multiplier)?
        .checked_add(fractional.checked_mul(multiplier)?.checked_div(scale)?)
}

fn bound_text(text: &str) -> String {
    let (prefix, truncated) = bound_prefix(text, MAX_TEXT_BYTES);
    if !truncated {
        return prefix.to_owned();
    }
    format!("{}…", prefix)
}

fn bound_prefix(text: &str, max_bytes: usize) -> (&str, bool) {
    if text.len() <= max_bytes {
        return (text, false);
    }
    let mut end = max_bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (&text[..end], true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct FakeRunner {
        outputs: Mutex<Vec<CommandOutput>>,
        seen: Mutex<Vec<CommandSpec>>,
    }

    impl FakeRunner {
        fn new(outputs: Vec<CommandOutput>) -> Self {
            Self {
                outputs: Mutex::new(outputs),
                seen: Mutex::new(Vec::new()),
            }
        }
    }

    impl CommandRunner for FakeRunner {
        fn run(&self, command: &CommandSpec) -> Result<CommandOutput, String> {
            self.seen.lock().unwrap().push(command.clone());
            self.outputs
                .lock()
                .unwrap()
                .pop()
                .ok_or_else(|| "missing fixture".into())
        }
    }

    fn output(stdout: &str, exit_code: Option<i32>) -> CommandOutput {
        CommandOutput {
            exit_code,
            stdout: stdout.into(),
            stderr: String::new(),
            timed_out: false,
            truncated: false,
        }
    }

    #[test]
    fn executable_allowlist_rejects_untrusted_basename_and_relative_components() {
        assert!(CommandSpec::fixed(Tool::Brew, Path::new("/tmp/not-brew"), &[], &[]).is_err());
        assert!(CommandSpec::fixed(Tool::Uv, Path::new("/usr/bin/../bin/uv"), &[], &[]).is_err());
    }

    #[test]
    fn homebrew_uses_only_dry_run_and_keeps_host_recovery_unknown() {
        let runner = FakeRunner::new(vec![output("Would remove: old-1 (12.5MB)\n", Some(0))]);
        let review = review_homebrew(&runner, Path::new("/opt/homebrew/bin/brew"));
        assert_eq!(review.state, ReviewState::Complete);
        assert_eq!(review.logical_recovery_bytes, 12_500_000);
        assert_eq!(review.host_recovery_bytes, None);
        assert_eq!(review.cleanup_authority, CleanupAuthority::ReviewOnly);
        let commands = runner.seen.lock().unwrap();
        assert_eq!(commands[0].args, ["cleanup", "--dry-run", "--verbose"]);
        assert!(!commands[0].args.iter().any(|arg| arg == "--scrub"));
    }

    #[test]
    fn uv_failure_is_unknown_and_never_invents_a_dry_run_prune() {
        let runner = FakeRunner::new(vec![output("", Some(2))]);
        let review = review_uv(&runner, Path::new("/usr/local/bin/uv"));
        assert_eq!(review.state, ReviewState::Unknown);
        assert!(
            !runner.seen.lock().unwrap()[0]
                .args
                .iter()
                .any(|arg| arg == "prune")
        );
    }

    #[test]
    fn pnpm_partial_store_status_does_not_claim_bytes() {
        let runner = FakeRunner::new(vec![
            output("modified package\n", Some(1)),
            output("/Users/test/.pnpm-store\n", Some(0)),
        ]);
        let review = review_pnpm(&runner, Path::new("/usr/local/bin/pnpm"));
        assert_eq!(review.state, ReviewState::Partial);
        assert_eq!(review.logical_recovery_bytes, 0);
        assert_eq!(review.host_recovery_bytes, None);
        assert!(review.owner_followup.contains("pnpm store prune"));
    }

    #[test]
    fn editor_metadata_is_never_promoted_to_cleanup_authority_or_recovery() {
        let mut report = serde_json::json!({
            "editor":"Vscode", "extensions_root":"/Users/test/.vscode/extensions",
            "marker_count":1, "verified_count":1, "complete":true, "reason":null,
            "records":[{"directory":"/Users/test/.vscode/extensions/company.example-1.2.3", "extension_id":"company.example", "version":"1.2.3", "marker":"company.example-1.2.3", "review_only":true}]
        });
        let run = |report: &serde_json::Value| {
            let runner = FakeRunner::new(vec![output(&report.to_string(), Some(0))]);
            let review = review_editor(
                &runner,
                Path::new("/bundle/Helpers/chippytea-scan-helper"),
                Path::new("/Users/test"),
                Provider::VsCodeExtensions,
            );
            assert_eq!(
                runner.seen.lock().unwrap()[0].args,
                ["--editor-review", "vscode"]
            );
            review
        };
        let review = run(&report);
        assert_eq!(review.state, ReviewState::Partial);
        assert_eq!(review.cleanup_authority, CleanupAuthority::ReviewOnly);
        assert_eq!(review.logical_recovery_bytes, 0);
        assert_eq!(review.host_recovery_bytes, None);
        assert_eq!(review.observations[0].logical_bytes, None);
        assert_eq!(review.observations[0].reclaimable, None);
        report["records"][0]["review_only"] = serde_json::json!(false);
        assert_eq!(run(&report).state, ReviewState::Unknown);
        report["records"][0]["review_only"] = serde_json::json!(true);
        report["records"][0]["directory"] =
            serde_json::json!("/Users/test/private/company.example-1.2.3");
        assert_eq!(run(&report).state, ReviewState::Unknown);
        report["records"][0]["directory"] =
            serde_json::json!("/Users/test/.vscode/extensions/company.example-1.2.3");
        report["editor"] = serde_json::json!("Cursor");
        assert_eq!(run(&report).state, ReviewState::Unknown);
    }

    #[test]
    fn docker_rejects_remote_context_before_daemon_runner() {
        let runner = FakeRunner::new(vec![output(
            r#"{"Host":"tcp://remote:2376","SkipTLSVerify":false}"#,
            Some(0),
        )]);
        let review = review_docker_installed(
            &runner,
            Path::new("/usr/local/bin/docker"),
            Path::new("/Users/test"),
            &AtomicBool::new(false),
        );
        assert_eq!(review.state, ReviewState::Unknown);
        assert_eq!(runner.seen.lock().unwrap().len(), 1);
    }

    #[test]
    fn docker_adapter_uses_one_metadata_cli_then_only_the_fixed_owned_get() {
        use std::io::{Read, Write};
        use std::os::unix::net::UnixListener;
        let temp = tempfile::tempdir_in("/tmp").unwrap();
        let home = std::fs::canonicalize(temp.path()).unwrap();
        let socket = home.join(".docker/run/docker.sock");
        std::fs::create_dir_all(socket.parent().unwrap()).unwrap();
        let listener = UnixListener::bind(&socket).unwrap();
        listener.set_nonblocking(true).unwrap();
        let server = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + Duration::from_secs(3);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            && std::time::Instant::now() < deadline =>
                    {
                        std::thread::sleep(Duration::from_millis(5))
                    }
                    Err(error) => panic!("disposable daemon did not receive its request: {error}"),
                }
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(1)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(1)))
                .unwrap();
            let mut request = Vec::new();
            let mut buffer = [0u8; 512];
            while !request.ends_with(b"\r\n\r\n") && request.len() < 2048 {
                let count = stream.read(&mut buffer).unwrap();
                assert!(count > 0);
                request.extend_from_slice(&buffer[..count]);
            }
            assert!(request.starts_with(b"GET /v1.46/system/df?type=build-cache HTTP/1.1\r\n"));
            let body = r#"{"BuildCache":[{"ID":"fixture-cache","Type":"regular","Description":"shared fixture","InUse":false,"Shared":true,"Size":41,"UsageCount":2}]}"#;
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
        });
        let context = serde_json::json!({"Host":format!("unix://{}", socket.display()),"SkipTLSVerify":false}).to_string();
        let runner = FakeRunner::new(vec![output(&context, Some(0))]);
        let review = review_docker_installed(
            &runner,
            Path::new("/usr/local/bin/docker"),
            &home,
            &AtomicBool::new(false),
        );
        server.join().unwrap();
        assert_eq!(review.state, ReviewState::Complete);
        assert_eq!(review.logical_recovery_bytes, 41);
        assert_eq!(review.host_recovery_bytes, None);
        assert_eq!(review.observations[0].reclaimable, None);
        assert_eq!(review.cleanup_authority, CleanupAuthority::ReviewOnly);
        let commands = runner.seen.lock().unwrap();
        assert_eq!(commands.len(), 1);
        assert_eq!(
            commands[0].args,
            [
                "context",
                "inspect",
                "default",
                "--format",
                "{{json .Endpoints.docker}}"
            ]
        );
        assert!(review.consequence.contains("not currently in use"));
        assert!(review.consequence.contains("Neither is a prune estimate"));
    }

    #[test]
    fn homebrew_ignores_summary_and_size_looking_filename_text() {
        let (observations, logical, partial) = parse_homebrew(
            "Homebrew cleanup summary\nWould remove: package-99MB (2.5MB)\nTotal reclaimed: 2.5MB\n",
            HOMEBREW_DOCS,
        );
        assert!(!partial);
        assert_eq!(logical, 2_500_000);
        assert_eq!(observations.len(), 3);
        assert_eq!(observations[1].logical_bytes, Some(2_500_000));
    }

    #[test]
    fn path_outputs_are_single_bounded_absolute_paths() {
        assert!(parse_single_absolute_path(&output("/Users/test/.cache/uv\n", Some(0))).is_some());
        assert!(parse_single_absolute_path(&output("relative\n", Some(0))).is_none());
        assert!(parse_single_absolute_path(&output("/tmp/a\n/tmp/b\n", Some(0))).is_none());
        assert!(
            parse_single_absolute_path(&CommandOutput {
                truncated: true,
                ..output("/tmp/a\n", Some(0))
            })
            .is_none()
        );
    }

    #[test]
    fn docker_endpoint_requires_real_context_shape() {
        assert_eq!(
            parse_docker_endpoint(
                r#"{"Host":"unix:///var/run/docker.sock","SkipTLSVerify":false}"#
            )
            .as_deref(),
            Some("unix:///var/run/docker.sock")
        );
        assert!(parse_docker_endpoint(r#""unix:///var/run/docker.sock""#).is_none());
        assert!(parse_docker_endpoint(r#"{"Host":"unix:///var/run/docker.sock"}"#).is_none());
        assert!(parse_docker_endpoint("{}\n{}").is_none());
    }

    #[test]
    fn homebrew_parses_file_counts_without_counting_summary_twice() {
        let (observations, bytes, partial) = parse_homebrew(
            "Would remove: /old/a (13 files, 5.3MB)\nWould remove: /old/b (1,073 files, 9.8MB)\nTotal reclaimed: 15.1MB\n",
            HOMEBREW_DOCS,
        );
        assert!(!partial);
        assert_eq!(bytes, 15_100_000);
        assert_eq!(observations[0].logical_bytes, Some(5_300_000));
        assert_eq!(observations[1].logical_bytes, Some(9_800_000));
        assert_eq!(observations[2].logical_bytes, None);
        assert!(parse_homebrew("Would remove: /old/a (unknown, 5MB)", HOMEBREW_DOCS).2);
    }

    #[test]
    fn storage_settings_are_preserved_without_inheriting_execution_or_remote_settings() {
        let settings = [
            ("UV_CACHE_DIR", "/custom/cache"),
            ("UV_CONFIG_FILE", "/custom/uv.toml"),
            ("XDG_CONFIG_HOME", "/custom/config"),
            ("PATH", "/untrusted"),
            ("DYLD_INSERT_LIBRARIES", "/untrusted.dylib"),
            ("DOCKER_HOST", "tcp://remote:2375"),
        ];
        let values = storage_environment(Tool::Uv, |key| {
            settings
                .iter()
                .find(|(name, _)| *name == key)
                .map(|(_, value)| (*value).into())
        })
        .unwrap();
        assert_eq!(values.len(), 3);
        assert!(values.contains(&("UV_CACHE_DIR".into(), "/custom/cache".into())));
        assert!(values.contains(&("UV_CONFIG_FILE".into(), "/custom/uv.toml".into())));
        assert!(
            storage_environment(Tool::Docker, |key| settings
                .iter()
                .find(|(name, _)| *name == key)
                .map(|(_, value)| (*value).into()))
            .unwrap()
            .is_empty()
        );
        let values = storage_environment(Tool::Brew, |key| match key {
            "HOMEBREW_CACHE" => Some("/custom/brew".into()),
            "HOMEBREW_CLEANUP_MAX_AGE_DAYS" => Some("180".into()),
            "HOMEBREW_NO_CLEANUP_FORMULAE" => Some("python@3.14,homebrew/core/node".into()),
            _ => None,
        })
        .unwrap();
        assert_eq!(values.len(), 3);
    }

    #[test]
    fn ambiguous_or_unsafe_storage_settings_do_not_fall_back_to_defaults() {
        for value in ["relative", "/tmp/../other", "/tmp/cache\n", ""] {
            assert!(
                storage_environment(Tool::Uv, |key| (key == "UV_CACHE_DIR")
                    .then(|| value.into()))
                .is_err()
            );
        }
        assert!(
            storage_environment(Tool::Pnpm, |key| match key {
                "npm_config_store_dir" => Some("/first".into()),
                "NPM_CONFIG_STORE_DIR" => Some("/second".into()),
                _ => None,
            })
            .is_err()
        );
    }

    #[test]
    fn installed_entry_points_support_homebrew_symlinks_without_arbitrary_scripts() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let temp = tempfile::tempdir().unwrap();
        let prefix = temp.path().join("installation");
        let bin = prefix.join("bin");
        let pnpm_bin = prefix.join("lib/node_modules/pnpm/bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(&pnpm_bin).unwrap();
        let target = pnpm_bin.join("pnpm.cjs");
        std::fs::write(&target, "fixture only").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755)).unwrap();
        symlink(&target, bin.join("pnpm")).unwrap();
        assert_eq!(
            resolve_candidate(Tool::Pnpm, &bin.join("pnpm"), &prefix),
            Some(std::fs::canonicalize(&target).unwrap())
        );
        assert!(resolve_candidate(Tool::Uv, &bin.join("pnpm"), &prefix).is_none());
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o777)).unwrap();
        assert!(resolve_candidate(Tool::Pnpm, &bin.join("pnpm"), &prefix).is_none());
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(resolve_candidate(Tool::Pnpm, &bin.join("pnpm"), &bin).is_none());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn real_sandbox_starts_and_reads_but_denies_writes_tcp_and_unix_sockets() {
        use std::net::TcpListener;
        use std::os::unix::net::UnixListener;
        fn run(program: &str, args: &[&str]) -> std::process::Output {
            let mut command = Command::new(SANDBOX_EXECUTABLE);
            command
                .args(["-p", sandbox_profile(), program])
                .args(args)
                .env_clear()
                .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
                .current_dir("/");
            probe::run(command, Duration::from_secs(3), 4096, None).unwrap()
        }
        assert!(
            run("/usr/bin/true", &[]).status.success(),
            "sandbox must actually start system binaries"
        );
        let temp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(temp.path()).unwrap();
        let readable = root.join("readable");
        std::fs::write(&readable, "read-only fixture").unwrap();
        assert_eq!(
            run("/bin/cat", &[readable.to_str().unwrap()]).stdout,
            b"read-only fixture"
        );
        let forbidden = root.join("must-not-exist");
        assert!(
            !run(
                "/bin/sh",
                &[
                    "-c",
                    "printf forbidden > \"$1\"",
                    "fixture",
                    forbidden.to_str().unwrap()
                ]
            )
            .status
            .success()
        );
        assert!(!forbidden.exists());
        let tcp = TcpListener::bind("127.0.0.1:0").unwrap();
        tcp.set_nonblocking(true).unwrap();
        let url = format!("http://{}/", tcp.local_addr().unwrap());
        assert!(
            !run(
                "/usr/bin/curl",
                &[
                    "-q",
                    "--silent",
                    "--show-error",
                    "--noproxy",
                    "*",
                    "--max-time",
                    "1",
                    &url
                ]
            )
            .status
            .success()
        );
        assert_eq!(
            tcp.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        let socket = root.join("fixture.sock");
        let unix = UnixListener::bind(&socket).unwrap();
        unix.set_nonblocking(true).unwrap();
        assert!(
            !run(
                "/usr/bin/curl",
                &[
                    "-q",
                    "--silent",
                    "--show-error",
                    "--noproxy",
                    "*",
                    "--max-time",
                    "1",
                    "--unix-socket",
                    socket.to_str().unwrap(),
                    "http://localhost/"
                ]
            )
            .status
            .success()
        );
        assert_eq!(
            unix.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }
}
