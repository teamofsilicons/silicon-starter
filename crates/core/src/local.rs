//! Small, stateful helpers used by the CLI. The public core types stay stateless;
//! this module only shells out to the user's installed git.
use crate::validate_silicon_yaml;
use serde::{Deserialize, Serialize};
use std::{
    fs, io,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Binding {
    pub id: String,
    pub api: String,
    pub mode: Mode,
    #[serde(default)]
    pub auto_update: bool,
    #[serde(default)]
    pub pinned: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RegistryEntry {
    pub path: PathBuf,
    pub binding: Binding,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Development,
    Download,
}

pub fn run_git(dir: impl AsRef<Path>, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .map_err(|e| format!("could not execute git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}
pub fn is_repo(dir: impl AsRef<Path>) -> bool {
    dir.as_ref().join(".git").exists()
}
pub fn binding_path(dir: impl AsRef<Path>) -> PathBuf {
    dir.as_ref().join(".git").join("starter.json")
}
pub fn load_binding(dir: impl AsRef<Path>) -> Result<Binding, String> {
    let p = binding_path(dir);
    let b =
        fs::read(&p).map_err(|_| format!("not a Starter checkout (missing {})", p.display()))?;
    serde_json::from_slice(&b).map_err(|e| format!("invalid Starter binding: {e}"))
}
pub fn save_binding(dir: impl AsRef<Path>, binding: &Binding) -> Result<(), String> {
    let p = binding_path(&dir);
    let bytes = serde_json::to_vec_pretty(binding).map_err(|e| e.to_string())?;
    fs::write(p, bytes).map_err(|e| e.to_string())
}
pub fn registry_path() -> PathBuf {
    std::env::var_os("SILICON_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".starter/registry.json")
}
pub fn register_checkout(dir: impl AsRef<Path>, binding: &Binding) -> Result<(), String> {
    let path = dir
        .as_ref()
        .canonicalize()
        .map_err(|e| format!("cannot resolve checkout: {e}"))?;
    let file = registry_path();
    if let Some(parent) = file.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut entries: Vec<RegistryEntry> = fs::read(&file)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    entries.retain(|x| x.path != path);
    entries.push(RegistryEntry {
        path,
        binding: binding.clone(),
    });
    let bytes = serde_json::to_vec_pretty(&entries).map_err(|e| e.to_string())?;
    atomic_write(&file, &bytes).map_err(|e| e.to_string())
}
pub fn ensure_main(dir: impl AsRef<Path>) -> Result<(), String> {
    let branch = run_git(&dir, ["branch", "--show-current"].as_ref())?;
    if branch != "main" {
        return Err(format!(
            "Starter operations require branch `main`; current branch is `{branch}`"
        ));
    }
    Ok(())
}
pub fn validate_checkout(dir: impl AsRef<Path>) -> Result<String, String> {
    let p = dir.as_ref().join("silicon.yaml");
    let meta = fs::symlink_metadata(&p)
        .map_err(|_| format!("missing base-dir silicon.yaml at {}", p.display()))?;
    if !meta.file_type().is_file() {
        return Err("silicon.yaml must be a regular file in the starter base directory".into());
    }
    let text = fs::read_to_string(&p).map_err(|e| format!("cannot read silicon.yaml: {e}"))?;
    validate_silicon_yaml(&text)?;
    Ok(text)
}
pub fn head(dir: impl AsRef<Path>) -> Result<String, String> {
    ensure_main(&dir)?;
    run_git(dir, ["rev-parse", "HEAD"].as_ref())
}
pub fn bundle(dir: impl AsRef<Path>) -> Result<(PathBuf, String), String> {
    let commit = head(&dir)?;
    let path = std::env::temp_dir().join(format!(
        "starter-{}-{}.bundle",
        std::process::id(),
        unique()
    ));
    run_git(
        &dir,
        [
            "bundle",
            "create",
            path.to_str().ok_or("temporary bundle path is not UTF-8")?,
            "refs/heads/main",
        ]
        .as_ref(),
    )?;
    Ok((path, commit))
}
pub fn unique() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}
pub fn clone_bundle(
    bundle: impl AsRef<Path>,
    target: impl AsRef<Path>,
    commit: &str,
) -> Result<(), String> {
    if !is_hex_commit(commit) {
        return Err("server returned an invalid commit id".into());
    }
    let target = target.as_ref();
    if target.exists()
        && fs::read_dir(target)
            .map_err(|e| e.to_string())?
            .next()
            .is_some()
    {
        return Err(format!(
            "target directory {} is not empty",
            target.display()
        ));
    }
    if target.exists() {
        fs::remove_dir_all(target).map_err(|e| e.to_string())?;
    }
    let parent = target.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let bundle = bundle.as_ref().to_str().ok_or("bundle path is not UTF-8")?;
    let target_s = target.to_str().ok_or("target path is not UTF-8")?;
    run_git(
        parent,
        [
            "-c",
            "protocol.file.allow=always",
            "clone",
            "--no-checkout",
            "--no-hardlinks",
            bundle,
            target_s,
        ]
        .as_ref(),
    )?;
    run_git(target, ["checkout", "-B", "main", commit].as_ref()).inspect_err(|_| {
        let _ = fs::remove_dir_all(target);
    })?;
    Ok(())
}
pub fn bundle_head(path: impl AsRef<Path>) -> Result<String, String> {
    let path = path.as_ref();
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let value = run_git(
        parent,
        [
            "bundle",
            "list-heads",
            path.to_str().ok_or("bundle path is not UTF-8")?,
        ]
        .as_ref(),
    )?;
    let commit = value
        .split_whitespace()
        .next()
        .ok_or("bundle contains no heads")?;
    if !is_hex_commit(commit) {
        return Err("bundle head is not a valid commit id".into());
    }
    Ok(commit.into())
}
pub fn is_hex_commit(s: &str) -> bool {
    (s.len() == 40 || s.len() == 64) && s.bytes().all(|b| b.is_ascii_hexdigit())
}
pub fn stage_and_commit(dir: impl AsRef<Path>, message: &str) -> Result<String, String> {
    if message.trim().is_empty() {
        return Err("commit message must not be empty".into());
    };
    ensure_main(&dir)?;
    run_git(&dir, ["add", "-A"].as_ref())?;
    let status = run_git(&dir, ["status", "--porcelain"].as_ref())?;
    if status.is_empty() {
        return Err("nothing to commit".into());
    };
    run_git(&dir, ["commit", "-m", message].as_ref())?;
    head(dir)
}
pub fn commit_history(dir: impl AsRef<Path>, limit: usize) -> Result<String, String> {
    run_git(
        dir,
        [
            "log",
            "--first-parent",
            &format!("-{limit}"),
            "--pretty=format:%H%x09%s%x09%cI",
        ]
        .as_ref(),
    )
}
pub fn repo_root() -> Result<PathBuf, String> {
    let s = run_git(".", ["rev-parse", "--show-toplevel"].as_ref())?;
    Ok(PathBuf::from(s))
}
pub fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension(format!("tmp-{}", unique()));
    fs::write(&tmp, bytes)?;
    fs::rename(tmp, path)
}
