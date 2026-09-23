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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_preview_preserves_order_and_expressions_without_running_them() {
        let temp = TempDir::new().unwrap();
        let marker = temp.0.join("must-not-exist");
        let content = include_str!("../../../starter_template/.starterbase/starter.yaml").replace(
            "node -p 'Intl.DateTimeFormat().resolvedOptions().timeZone'",
            &format!("touch '{}'", marker.display()),
        );
        let mut files = vec![RepositoryFile {
            path: ".starterbase/starter.yaml".into(),
            size: content.len() as u64,
            kind: "file",
            content: Some(content),
            reason: None,
        }];
        let (preview, error) = template_preview(&files);
        assert!(error.is_none(), "{error:?}");
        let preview = preview.unwrap();
        let variables = preview["variables"].as_array().unwrap();
        assert_eq!(
            variables
                .iter()
                .map(|value| value["name"].as_str().unwrap())
                .collect::<Vec<_>>(),
            [
                "silicon_id",
                "silicon_org_id",
                "silicon_token",
                "timezone",
                "purpose",
                "waveform",
                "waveform_tts_provider"
            ]
        );
        assert!(
            variables[3]["default"]
                .as_str()
                .unwrap()
                .starts_with("! touch")
        );
        assert_eq!(variables[6]["when"], "{var.waveform}");
        assert_eq!(preview["build"][0], "! sh build.sh");
        assert!(!marker.exists());
        files[0].content = Some("schema: invalid".into());
        let (preview, error) = template_preview(&files);
        assert!(preview.is_none() && error.is_some());
        files[0].content = None;
        files[0].reason = Some("large");
        assert!(template_preview(&files).1.unwrap().contains("large"));
        assert_eq!(template_preview(&[]), (None, None));
    }

    #[test]
    fn reads_real_bundle_at_exact_commit_without_checkout() {
        let temp = TempDir::new().unwrap();
        let root = &temp.0;
        git(root, &["init", "--quiet"], MAX_TREE_BYTES).unwrap();
        git(root, &["config", "user.name", "Test"], 1024).unwrap();
        git(
            root,
            &["config", "user.email", "test@example.invalid"],
            1024,
        )
        .unwrap();
        fs::create_dir(root.join("nested")).unwrap();
        fs::write(root.join("silicon.yaml"), "silicon: first\n").unwrap();
        fs::write(root.join("nested/space and\nnewline.txt"), "nested text\n").unwrap();
        fs::write(root.join("binary.dat"), [0, 159, 255]).unwrap();
        fs::write(root.join("empty"), []).unwrap();
        fs::write(root.join("large"), vec![b'a'; MAX_FILE_BYTES + 1]).unwrap();
        for index in 0..9 {
            fs::write(
                root.join(format!("z-budget-{index}")),
                vec![b'z'; MAX_FILE_BYTES],
            )
            .unwrap();
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink("/etc/passwd", root.join("link")).unwrap();
        git(root, &["add", "."], 1024).unwrap();
        git(root, &["commit", "--quiet", "-m", "first"], 1024).unwrap();
        let commit = String::from_utf8(git(root, &["rev-parse", "HEAD"], 128).unwrap())
            .unwrap()
            .trim()
            .to_owned();
        fs::write(root.join("silicon.yaml"), "silicon: second\n").unwrap();
        git(root, &["commit", "--quiet", "-am", "second"], 1024).unwrap();
        git(root, &["bundle", "create", "release.bundle", "--all"], 1024).unwrap();
        let bundle = fs::read(root.join("release.bundle")).unwrap();
        let result = read_bundle(bundle.clone(), commit.clone()).unwrap();
        let file = |path| result.files.iter().find(|file| file.path == path).unwrap();
        assert_eq!(result.commit.as_deref(), Some(commit.as_str()));
        assert!(!result.draft && !result.truncated);
        assert_eq!(
            file("silicon.yaml").content.as_deref(),
            Some("silicon: first\n")
        );
        assert_eq!(
            file("nested/space and\nnewline.txt").content.as_deref(),
            Some("nested text\n")
        );
        assert_eq!(file("empty").content.as_deref(), Some(""));
        assert_eq!(file("binary.dat").size, 3);
        assert_eq!(file("binary.dat").reason, Some("binary"));
        assert_eq!(file("large").reason, Some("large"));
        assert_eq!(file("z-budget-8").reason, Some("response_limit"));
        assert!(
            result
                .files
                .iter()
                .filter_map(|file| file.content.as_ref())
                .map(String::len)
                .sum::<usize>()
                <= MAX_CONTENT_BYTES
        );
        #[cfg(unix)]
        {
            assert_eq!(file("link").kind, "symlink");
            assert_eq!(file("link").reason, Some("symlink"));
            assert!(file("link").content.is_none());
        }
        assert!(read_bundle(b"invalid bundle".to_vec(), commit).is_err());
        assert!(read_bundle(bundle.clone(), "0".repeat(40)).is_err());
        assert!(read_bundle(bundle, "HEAD".into()).is_err());
        let empty = draft(String::new());
        assert!(empty.draft && empty.commit.is_none());
        assert_eq!(empty.files[0].content.as_deref(), Some(""));
        git(root, &["rm", "-r", "--quiet", "."], 1024).unwrap();
        git(root, &["commit", "--quiet", "-m", "empty tree"], 1024).unwrap();
        let empty_commit = String::from_utf8(git(root, &["rev-parse", "HEAD"], 128).unwrap())
            .unwrap()
            .trim()
            .to_owned();
        git(root, &["bundle", "create", "empty.bundle", "HEAD"], 1024).unwrap();
        assert!(
            read_bundle(fs::read(root.join("empty.bundle")).unwrap(), empty_commit)
                .unwrap()
                .files
                .is_empty()
        );
        let path = root.clone();
        drop(temp);
        assert!(!path.exists());
    }
}
