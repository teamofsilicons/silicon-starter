//! Read Git objects without a working tree, filters, or symlink traversal.
use serde::Serialize;
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
use uuid::Uuid;

const MAX_BUNDLE_BYTES: usize = 32 * 1024 * 1024;
const MAX_TREE_BYTES: usize = 1024 * 1024;
const MAX_FILES: usize = 500;
const MAX_FILE_BYTES: usize = 128 * 1024;
const MAX_CONTENT_BYTES: usize = 1024 * 1024;

#[derive(Serialize)]
pub struct RepositoryFiles {
    pub commit: Option<String>,
    pub draft: bool,
    pub truncated: bool,
    pub files: Vec<RepositoryFile>,
    pub template: Option<serde_json::Value>,
    pub template_error: Option<String>,
}

#[derive(Serialize)]
pub struct RepositoryFile {
    pub path: String,
    pub size: u64,
    pub kind: &'static str,
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<&'static str>,
}

pub fn draft(yaml: String) -> RepositoryFiles {
    let size = yaml.len() as u64;
    let large = yaml.len() > MAX_FILE_BYTES;
    RepositoryFiles {
        commit: None,
        draft: true,
        truncated: false,
        files: vec![RepositoryFile {
            path: "silicon.yaml".into(),
            size,
            kind: "file",
            content: (!large).then_some(yaml),
            reason: large.then_some("large"),
        }],
        template: None,
        template_error: None,
    }
}

// Parse author-supplied metadata only. Viewing a starter must never resolve answers or run scripts.
fn template_preview(files: &[RepositoryFile]) -> (Option<serde_json::Value>, Option<String>) {
    let Some(file) = files
        .iter()
        .find(|file| file.path == ".starterbase/starter.yaml")
    else {
        return (None, None);
    };
    let Some(content) = &file.content else {
        return (
            None,
            Some(format!(
                "starter.yaml cannot be read as text ({})",
                file.reason.unwrap_or("unavailable")
            )),
        );
    };
    let result = silicon_starter_core::template::parse_recipe(content).and_then(|recipe| {
        let mut preview = serde_json::to_value(&recipe).map_err(|error| error.to_string())?;
        let variables = recipe
            .variables
            .iter()
            .map(|(name, variable)| {
                let mut value =
                    serde_json::to_value(variable).map_err(|error| error.to_string())?;
                value["name"] = serde_json::Value::String(name.clone());
                Ok(value)
            })
            .collect::<Result<Vec<_>, String>>()?;
        preview["variables"] = serde_json::Value::Array(variables);
        Ok(preview)
    });
    match result {
        Ok(preview) => (Some(preview), None),
        Err(error) => (None, Some(error)),
    }
}

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Result<Self, &'static str> {
        let path = std::env::temp_dir().join(format!("starter-files-{}", Uuid::now_v7()));
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder
            .create(&path)
            .map_err(|_| "cannot create repository workspace")?;
        Ok(Self(path))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn git(repo: &Path, args: &[&str], limit: usize) -> Result<Vec<u8>, &'static str> {
    let mut child = Command::new("git")
        .args(["-c", "core.hooksPath=/dev/null", "-c", "init.templateDir="])
        .args(args)
        .current_dir(repo)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "git is unavailable")?;
    let mut output = Vec::new();
    let read = child
        .stdout
        .take()
        .unwrap()
        .take(limit as u64 + 1)
        .read_to_end(&mut output);
    if read.is_err() || output.len() > limit {
        let _ = child.kill();
        let _ = child.wait();
        return Err("repository exceeds the browsing limit");
    }
    if !child
        .wait()
        .map_err(|_| "cannot read git process status")?
        .success()
    {
        return Err("repository bundle or commit is invalid");
    }
    Ok(output)
}

pub fn read_bundle(bundle: Vec<u8>, commit: String) -> Result<RepositoryFiles, &'static str> {
    if bundle.len() > MAX_BUNDLE_BYTES {
        return Err("repository bundle exceeds the browsing limit");
    }
    if !matches!(commit.len(), 40 | 64) || !commit.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("repository has no valid full commit id");
    }
    let temp = TempDir::new()?;
    let repo = temp.0.join("repo.git");
    let bundle_path = temp.0.join("repository.bundle");
    fs::write(&bundle_path, bundle).map_err(|_| "cannot stage repository bundle")?;
    let format = if commit.len() == 64 {
        "--object-format=sha256"
    } else {
        "--object-format=sha1"
    };
    git(
        &temp.0,
        &["init", "--bare", "--quiet", format, "repo.git"],
        MAX_TREE_BYTES,
    )?;
    git(
        &repo,
        &["bundle", "unbundle", "../repository.bundle"],
        MAX_TREE_BYTES,
    )?;
    if git(&repo, &["cat-file", "-t", &commit], 16)? != b"commit\n" {
        return Err("repository reference is not a commit");
    }
    let tree = git(&repo, &["ls-tree", "-rlz", &commit], MAX_TREE_BYTES)?;
    let mut files = Vec::new();
    let mut content_bytes = 0;
    let mut truncated = false;
    // ponytail: preview at most 500 files / 1 MiB; add paged blob endpoints for larger repositories.
    for entry in tree.split(|b| *b == 0).filter(|entry| !entry.is_empty()) {
        if files.len() == MAX_FILES {
            truncated = true;
            break;
        }
        let tab = entry
            .iter()
            .position(|b| *b == b'\t')
            .ok_or("invalid git tree entry")?;
        let fields: Vec<_> = std::str::from_utf8(&entry[..tab])
            .map_err(|_| "invalid git tree entry")?
            .split_whitespace()
            .collect();
        if fields.len() != 4 {
            return Err("invalid git tree entry");
        }
        let path = std::str::from_utf8(&entry[tab + 1..])
            .map_err(|_| "repository filenames must be UTF-8")?
            .to_owned();
        let submodule = fields[1] == "commit";
        let size = if submodule {
            0
        } else {
            fields[3]
                .parse::<u64>()
                .map_err(|_| "invalid git blob size")?
        };
        let (kind, mut reason) = match fields[0] {
            "120000" => ("symlink", Some("symlink")),
            "160000" => ("submodule", Some("submodule")),
            "100644" | "100755" => ("file", None),
            _ => return Err("unsupported git tree entry"),
        };
        let mut content = None;
        if reason.is_none() {
            if size > MAX_FILE_BYTES as u64 {
                reason = Some("large");
            } else if content_bytes + size as usize > MAX_CONTENT_BYTES {
                reason = Some("response_limit");
            } else {
                let bytes = git(&repo, &["cat-file", "blob", fields[2]], MAX_FILE_BYTES)?;
                if bytes.len() as u64 != size {
                    return Err("git blob size does not match the tree");
                }
                if bytes.contains(&0) {
                    reason = Some("binary");
                } else if let Ok(text) = String::from_utf8(bytes) {
                    content_bytes += text.len();
                    content = Some(text);
                } else {
                    reason = Some("binary");
                }
            }
        }
        files.push(RepositoryFile {
            path,
            size,
            kind,
            content,
            reason,
        });
    }
    let (template, template_error) = template_preview(&files);
    Ok(RepositoryFiles {
        commit: Some(commit),
        draft: false,
        truncated,
        files,
        template,
        template_error,
    })
}
