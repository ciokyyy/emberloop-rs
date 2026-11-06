//! Versioning and self-update.
//!
//! A release publishes a bare binary per target plus one `latest.json` beside it:
//!
//! ```json
//! { "version": "0.0.4",
//!   "targets": { "x86_64-unknown-linux-gnu": { "url": "https://…/emberloop-0.0.4-x86_64-unknown-linux-gnu",
//!                                              "sha256": "…" } } }
//! ```
//!
//! `emberloop update` reads that file, downloads the artifact for this target, checks its sha256,
//! and keeps it as `releases/<version>/`. `current` is a symlink to one release and is switched
//! atomically, so the running binary is never overwritten, old releases stay for `--rollback`, and a
//! bad download never replaces a working build. Nothing is unpacked, so no `tar` or `flate2`.
//!
//! The Session that asked for the update is written to `update-pending.json` first, so the daemon
//! that starts afterwards can say which Session it is resuming.

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The version this build was compiled as.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

const DEFAULT_TIMEOUT_SECS: u64 = 120;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Artifact {
    pub url: String,
    pub sha256: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Latest {
    pub version: String,
    #[serde(default)]
    pub targets: BTreeMap<String, Artifact>,
}

impl Latest {
    /// The artifact for the target this binary was built for, if the release has one.
    pub fn artifact(&self) -> Option<&Artifact> {
        self.targets.get(&target())
    }
}

/// target is the triple the release names its artifacts by: `<arch>-unknown-<os>-<env>`.
pub fn target() -> String {
    let arch = std::env::consts::ARCH;
    match std::env::consts::OS {
        "linux" => format!("{arch}-unknown-linux-gnu"),
        "macos" => format!("{arch}-apple-darwin"),
        "windows" => format!("{arch}-pc-windows-msvc"),
        other => format!("{arch}-unknown-{other}"),
    }
}

/// compare orders two dotted versions by their numeric parts; a pre-release suffix sorts below the
/// release it belongs to, which is all this needs.
pub fn compare(left: &str, right: &str) -> Ordering {
    let split = |value: &str| -> (Vec<u64>, bool) {
        let (core, pre) = match value.split_once('-') {
            Some((core, _)) => (core, true),
            None => (value, false),
        };
        let parts = core
            .split('.')
            .map(|part| part.trim().parse::<u64>().unwrap_or(0))
            .collect();
        (parts, pre)
    };
    let (mut a, pre_a) = split(left.trim());
    let (mut b, pre_b) = split(right.trim());
    a.resize(a.len().max(b.len()), 0);
    b.resize(b.len().max(a.len()), 0);
    match a.cmp(&b) {
        Ordering::Equal => pre_b.cmp(&pre_a),
        other => other,
    }
}

/// is_newer says whether `latest` is worth installing over `current`.
pub fn is_newer(latest: &str, current: &str) -> bool {
    compare(latest, current) == Ordering::Greater
}

pub fn default_manifest_url() -> String {
    std::env::var("EMBERLOOP_UPDATE_URL").unwrap_or_else(|_| {
        "https://github.com/ciokyyy/emberloop-rs/releases/latest/download/latest.json".to_string()
    })
}

/// fetch reads the release manifest.
pub async fn fetch(manifest_url: &str) -> anyhow::Result<Latest> {
    let body = client()?
        .get(manifest_url)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    Ok(serde_json::from_str(&body)?)
}

fn client() -> anyhow::Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .timeout(Duration::from_secs(DEFAULT_TIMEOUT_SECS))
        .user_agent(format!("emberloop/{VERSION}"))
        .build()?)
}

/// BINARY is the file name every release directory holds.
const BINARY: &str = "emberloop-daemon";

/// install downloads `artifact`, checks its sum and keeps it as `releases/<version>/` under
/// `root`. Nothing running is touched; `activate` switches to it.
pub async fn install(artifact: &Artifact, version: &str, root: &Path) -> anyhow::Result<()> {
    let bytes = client()?
        .get(&artifact.url)
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;
    let sum = format!("{:x}", Sha256::digest(&bytes));
    anyhow::ensure!(
        sum == artifact.sha256.to_ascii_lowercase(),
        "download checksum {sum} does not match the manifest ({})",
        artifact.sha256
    );
    store_release(root, version, &bytes)?;
    Ok(())
}

pub fn releases_dir(root: &Path) -> PathBuf {
    root.join("releases")
}

pub fn release_binary(root: &Path, version: &str) -> PathBuf {
    releases_dir(root).join(version).join(BINARY)
}

pub fn current_binary(root: &Path) -> PathBuf {
    root.join("current").join(BINARY)
}

/// store_release writes the binary into a scratch directory and renames that into place, so a
/// release directory is either whole or absent. A version already stored is left as it is.
pub fn store_release(root: &Path, version: &str, bytes: &[u8]) -> anyhow::Result<PathBuf> {
    let final_dir = releases_dir(root).join(version);
    if final_dir.exists() {
        return Ok(final_dir);
    }
    let scratch = releases_dir(root).join(format!("{version}.partial.{}", std::process::id()));
    std::fs::create_dir_all(&scratch)?;
    let result = write_binary(&scratch.join(BINARY), bytes)
        .and_then(|()| std::fs::rename(&scratch, &final_dir).map_err(Into::into));
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&scratch);
    }
    result.map(|()| final_dir)
}

fn write_binary(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    use std::io::Write;
    let mut file = std::fs::File::create(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
}

/// point_at makes `link` a symlink to `target` by building a sibling link and renaming it over,
/// so readers see the old target or the new one and never a gap.
#[cfg(unix)]
fn point_at(link: &Path, target: &Path) -> anyhow::Result<()> {
    let tmp = link.with_extension(format!("tmp.{}", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    std::os::unix::fs::symlink(target, &tmp)?;
    if let Err(error) = std::fs::rename(&tmp, link) {
        let _ = std::fs::remove_file(&tmp);
        return Err(error.into());
    }
    Ok(())
}

#[cfg(not(unix))]
fn point_at(_link: &Path, _target: &Path) -> anyhow::Result<()> {
    anyhow::bail!("releases are switched with symlinks, which this platform does not support")
}

/// activate switches `current` to a stored release; a missing release changes nothing.
pub fn activate(root: &Path, version: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        release_binary(root, version).is_file(),
        "release {version} is not installed"
    );
    point_at(&root.join("current"), &Path::new("releases").join(version))
}

/// link_launcher makes `launcher` (the path people and units run) follow `current`.
pub fn link_launcher(launcher: &Path, root: &Path) -> anyhow::Result<()> {
    let target = current_binary(root);
    if std::fs::read_link(launcher).is_ok_and(|now| now == target) {
        return Ok(());
    }
    if let Some(parent) = launcher.parent() {
        std::fs::create_dir_all(parent)?;
    }
    point_at(launcher, &target)
}

pub fn current_version(root: &Path) -> Option<String> {
    let target = std::fs::read_link(root.join("current")).ok()?;
    Some(target.file_name()?.to_string_lossy().into_owned())
}

/// list_releases names the stored versions, oldest first.
pub fn list_releases(root: &Path) -> Vec<String> {
    let mut versions: Vec<String> = std::fs::read_dir(releases_dir(root))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.path().join(BINARY).is_file())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    versions.sort_by(|a, b| compare(a, b));
    versions
}

fn previous_of(versions: &[String], version: &str) -> Option<String> {
    let at = versions.iter().position(|each| each == version)?;
    versions.get(at.checked_sub(1)?).cloned()
}

/// rollback points `current` at the release before it and returns that version.
pub fn rollback(root: &Path) -> anyhow::Result<String> {
    let current = current_version(root).ok_or_else(|| anyhow::anyhow!("no current release"))?;
    let previous = previous_of(&list_releases(root), &current)
        .ok_or_else(|| anyhow::anyhow!("no release older than {current}"))?;
    activate(root, &previous)?;
    Ok(previous)
}

/// prune removes old releases, keeping the newest `keep`, the current one and the one before it
/// (the rollback target).
pub fn prune(root: &Path, keep: usize) -> anyhow::Result<()> {
    let versions = list_releases(root);
    let mut kept: Vec<String> = versions.iter().rev().take(keep).cloned().collect();
    if let Some(current) = current_version(root) {
        kept.extend(previous_of(&versions, &current));
        kept.push(current);
    }
    for version in versions.iter().filter(|each| !kept.contains(each)) {
        std::fs::remove_dir_all(releases_dir(root).join(version))?;
    }
    Ok(())
}

/// The Session an `emberloop update` runs inside, found from the directory the bash tool exports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InSession {
    pub id: String,
    pub data_root: PathBuf,
}

/// in_session reads `EMBERLOOP_SESSION_DIR` (`<data root>/sessions/<id>`).
pub fn in_session(session_dir: Option<&Path>) -> Option<InSession> {
    let dir = session_dir?;
    let sessions = dir.parent()?;
    if sessions.file_name()? != "sessions" {
        return None;
    }
    Some(InSession {
        id: dir.file_name()?.to_string_lossy().into_owned(),
        data_root: sessions.parent()?.to_path_buf(),
    })
}

/// installed_message is what the CLI prints once the new release is active. Inside a Session the
/// reader is the model, which has to go idle before the daemon can restart under it.
pub fn installed_message(from: &str, to: &str, in_session: bool) -> String {
    if in_session {
        format!(
            "Update {from} -> {to} is queued. End your turn now: stop calling tools and reply with one short line. \
             The daemon installs it once you are idle, restarts, and prompts you again when the new build is up."
        )
    } else {
        format!("Binary updated {from} -> {to}; restart the daemon to run it.")
    }
}

/// What a model inside a Session asked for: the daemon installs it once everything is idle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallRequest {
    pub to: String,
    pub artifact: Artifact,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
}

fn request_path(data_root: &Path) -> PathBuf {
    data_root.join("update-request.json")
}

pub fn write_request(data_root: &Path, request: &InstallRequest) -> std::io::Result<()> {
    let path = request_path(data_root);
    let tmp = path.with_extension(format!("json.tmp.{}", std::process::id()));
    std::fs::write(&tmp, serde_json::to_vec_pretty(request)?)?;
    std::fs::rename(&tmp, &path)
}

pub fn read_request(data_root: &Path) -> Option<InstallRequest> {
    serde_json::from_slice(&std::fs::read(request_path(data_root)).ok()?).ok()
}

/// apply_request installs the requested release, switches `current` to it and leaves the marker
/// the next daemon start announces. The request is consumed either way, so a bad download is
/// reported once instead of being retried at every turn end.
pub async fn apply_request(root: &Path, data_root: &Path) -> anyhow::Result<Option<PendingUpdate>> {
    let Some(request) = read_request(data_root) else {
        return Ok(None);
    };
    let _ = std::fs::remove_file(request_path(data_root));
    install(&request.artifact, &request.to, root).await?;
    activate(root, &request.to)?;
    prune(root, 3)?;
    let pending = PendingUpdate {
        from: VERSION.to_string(),
        to: request.to,
        session: request.session,
    };
    write_pending(data_root, &pending)?;
    Ok(Some(pending))
}

/// reexec replaces this process with `current`, same arguments and environment. It only returns
/// on failure.
#[cfg(unix)]
pub fn reexec(root: &Path) -> std::io::Error {
    use std::os::unix::process::CommandExt;
    std::process::Command::new(current_binary(root))
        .args(std::env::args_os().skip(1))
        .exec()
}

/// The marker left for the daemon that starts after a swap.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PendingUpdate {
    pub from: String,
    pub to: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
}

impl PendingUpdate {
    /// The event payload a finished swap reports; the notice row reads `text` and `route`.
    pub fn notice(&self) -> serde_json::Value {
        let text = format!(
            "Updated {} → {}. This Session resumes on the new build.",
            self.from, self.to
        );
        serde_json::json!({ "text": text, "route": "injected" })
    }
}

/// The line the re-prompted Session reads when the new build is up.
pub fn resume_prompt(pending: &PendingUpdate) -> String {
    format!(
        "Updated {} -> {}; the daemon restarted on the new build. Continue where you left off.",
        pending.from, pending.to
    )
}

fn env_path(name: &str, fallback: &str) -> PathBuf {
    std::env::var_os(name)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_default()
                .join(fallback)
        })
}

/// install_root holds `releases/` and `current`.
pub fn install_root() -> PathBuf {
    env_path("EMBERLOOP_HOME", ".local/share/emberloop-rs")
}

/// launcher_path is the path people and units run; it follows `current`.
pub fn launcher_path() -> PathBuf {
    env_path("EMBERLOOP_LAUNCHER", ".local/bin/emberloop-daemon")
}

pub fn pending_path(data_root: &Path) -> PathBuf {
    data_root.join("update-pending.json")
}

pub fn write_pending(data_root: &Path, pending: &PendingUpdate) -> std::io::Result<()> {
    let path = pending_path(data_root);
    let tmp = path.with_extension(format!("json.tmp.{}", std::process::id()));
    std::fs::write(&tmp, serde_json::to_vec_pretty(pending)?)?;
    std::fs::rename(&tmp, &path)
}

/// take_pending reads the marker and deletes it, so an update is announced exactly once.
pub fn take_pending(data_root: &Path) -> Option<PendingUpdate> {
    let path = pending_path(data_root);
    let bytes = std::fs::read(&path).ok()?;
    let _ = std::fs::remove_file(&path);
    serde_json::from_slice(&bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_order_by_their_numbers() {
        assert!(is_newer("0.0.10", "0.0.9"));
        assert!(is_newer("0.1.0", "0.0.99"));
        assert!(!is_newer("0.0.4", "0.0.4"));
        assert!(!is_newer("0.0.3", "0.0.4"));
        assert!(is_newer("1.0.0", "1.0.0-rc.1"));
        assert!(!is_newer("1.0.0-rc.1", "1.0.0"));
    }

    #[test]
    fn a_manifest_names_the_artifact_for_this_target() {
        let latest: Latest = serde_json::from_str(
            r#"{"version":"9.9.9","targets":{"x86_64-unknown-linux-gnu":{"url":"u","sha256":"s"}}}"#,
        )
        .unwrap();
        assert!(is_newer(&latest.version, VERSION));
        let Some(artifact) = latest.artifact() else {
            return;
        };
        assert_eq!(artifact.url, "u");
    }

    #[test]
    fn a_pending_marker_is_read_once() {
        let dir = tempfile::tempdir().unwrap();
        let pending = PendingUpdate {
            from: "0.0.3".into(),
            to: "0.0.4".into(),
            session: Some("ses_1".into()),
        };
        write_pending(dir.path(), &pending).unwrap();
        assert_eq!(take_pending(dir.path()), Some(pending));
        assert_eq!(take_pending(dir.path()), None);
    }

    #[test]
    fn a_session_dir_names_the_session_and_its_data_root() {
        let found = in_session(Some(Path::new("/data/sessions/ses_abc"))).unwrap();
        assert_eq!(found.id, "ses_abc");
        assert_eq!(found.data_root, Path::new("/data"));
        assert!(in_session(Some(Path::new("/data/elsewhere/ses_abc"))).is_none());
        assert!(in_session(None).is_none());
    }

    #[test]
    fn inside_a_session_the_report_tells_the_model_to_end_its_turn() {
        let inside = installed_message("0.0.3", "0.0.4", true);
        assert!(inside.contains("0.0.4"), "{inside}");
        assert!(inside.contains("End your turn now"), "{inside}");
        let outside = installed_message("0.0.3", "0.0.4", false);
        assert!(!outside.contains("End your turn"), "{outside}");
        assert!(outside.contains("restart"), "{outside}");
    }

    #[test]
    fn releases_are_kept_and_current_points_at_the_newest() {
        let root = tempfile::tempdir().unwrap();
        store_release(root.path(), "0.0.3", b"three").unwrap();
        activate(root.path(), "0.0.3").unwrap();
        store_release(root.path(), "0.0.4", b"four").unwrap();
        activate(root.path(), "0.0.4").unwrap();
        assert_eq!(current_version(root.path()).as_deref(), Some("0.0.4"));
        assert_eq!(
            std::fs::read(release_binary(root.path(), "0.0.3")).unwrap(),
            b"three"
        );
        assert_eq!(std::fs::read(current_binary(root.path())).unwrap(), b"four");
    }

    #[test]
    fn rollback_points_current_at_the_previous_release() {
        let root = tempfile::tempdir().unwrap();
        for version in ["0.0.2", "0.0.3", "0.0.4"] {
            store_release(root.path(), version, version.as_bytes()).unwrap();
            activate(root.path(), version).unwrap();
        }
        assert_eq!(rollback(root.path()).unwrap(), "0.0.3");
        assert_eq!(current_version(root.path()).as_deref(), Some("0.0.3"));
    }

    #[test]
    fn pruning_keeps_the_newest_and_never_the_current_or_previous_away() {
        let root = tempfile::tempdir().unwrap();
        for version in ["0.0.1", "0.0.2", "0.0.3", "0.0.4", "0.0.5"] {
            store_release(root.path(), version, b"x").unwrap();
        }
        activate(root.path(), "0.0.1").unwrap();
        activate(root.path(), "0.0.2").unwrap();
        prune(root.path(), 2).unwrap();
        let kept = list_releases(root.path());
        assert_eq!(kept, ["0.0.1", "0.0.2", "0.0.4", "0.0.5"]);
    }

    #[test]
    fn a_failed_activation_leaves_current_alone() {
        let root = tempfile::tempdir().unwrap();
        store_release(root.path(), "0.0.3", b"three").unwrap();
        activate(root.path(), "0.0.3").unwrap();
        assert!(activate(root.path(), "9.9.9").is_err());
        assert_eq!(current_version(root.path()).as_deref(), Some("0.0.3"));
    }

    async fn serve_bytes(bytes: &'static [u8]) -> String {
        let app =
            axum::Router::new().route("/bin", axum::routing::get(move || async move { bytes }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/bin", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        url
    }

    fn request_for(url: String, sha256: String) -> InstallRequest {
        InstallRequest {
            to: "0.0.9".into(),
            artifact: Artifact { url, sha256 },
            session: Some("ses_1".into()),
        }
    }

    #[tokio::test]
    async fn applying_a_request_installs_activates_and_leaves_the_marker() {
        let root = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let sum = format!("{:x}", Sha256::digest(b"nine"));
        let url = serve_bytes(b"nine").await;
        write_request(data.path(), &request_for(url, sum)).unwrap();
        let applied = apply_request(root.path(), data.path())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(applied.to, "0.0.9");
        assert_eq!(current_version(root.path()).as_deref(), Some("0.0.9"));
        assert_eq!(std::fs::read(current_binary(root.path())).unwrap(), b"nine");
        let pending = take_pending(data.path()).unwrap();
        assert_eq!(pending.session.as_deref(), Some("ses_1"));
        assert!(read_request(data.path()).is_none());
    }

    #[tokio::test]
    async fn a_checksum_mismatch_installs_nothing_and_keeps_current() {
        let root = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        store_release(root.path(), "0.0.3", b"three").unwrap();
        activate(root.path(), "0.0.3").unwrap();
        let url = serve_bytes(b"tampered").await;
        write_request(data.path(), &request_for(url, "00".repeat(32))).unwrap();
        assert!(apply_request(root.path(), data.path()).await.is_err());
        assert_eq!(current_version(root.path()).as_deref(), Some("0.0.3"));
        assert!(!release_binary(root.path(), "0.0.9").exists());
        assert!(take_pending(data.path()).is_none());
    }

    #[test]
    fn the_resume_prompt_names_both_versions() {
        let pending = PendingUpdate {
            from: "0.0.3".into(),
            to: "0.0.4".into(),
            session: None,
        };
        let text = resume_prompt(&pending);
        assert!(text.contains("0.0.3") && text.contains("0.0.4"), "{text}");
    }

    #[test]
    fn no_request_means_nothing_to_apply() {
        let data = tempfile::tempdir().unwrap();
        assert!(read_request(data.path()).is_none());
    }
}
