use clap::Args;
use serde_json::{Map, Value};
use silicon_starter_core::{local, seed, template};
use std::{
    collections::BTreeMap,
    fs,
    io::{IsTerminal, Write},
    path::{Path, PathBuf},
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Args, Default)]
pub struct SeedOptions {
    /// Use saved answers and defaults without terminal questions.
    #[arg(long)]
    pub defaults: bool,
    /// Supply an answer as NAME=JSON, or NAME=text for a string. Repeatable.
    #[arg(long = "set", value_name = "NAME=VALUE")]
    values: Vec<String>,
    /// Read explicit answers from a JSON object (useful for secrets).
    #[arg(long, value_name = "FILE")]
    answers: Option<PathBuf>,
    /// Forget a saved answer and resolve its current default. Repeatable.
    #[arg(long, value_name = "NAME")]
    reset: Vec<String>,
}

impl SeedOptions {
    pub fn unattended() -> Self {
        Self {
            defaults: true,
            ..Default::default()
        }
    }
    pub fn interactive(&self) -> bool {
        !self.defaults && std::io::stdin().is_terminal()
    }
    fn answers(&self) -> Result<Map<String, Value>> {
        let mut answers = if let Some(path) = &self.answers {
            serde_json::from_slice::<Map<String, Value>>(&fs::read(path)?)?
        } else {
            Map::new()
        };
        for pair in &self.values {
            let (key, value) = pair.split_once('=').ok_or("--set expects NAME=VALUE")?;
            if key.is_empty() {
                return Err("--set requires a variable name".into());
            }
            answers.insert(
                key.into(),
                serde_json::from_str(value).unwrap_or_else(|_| Value::String(value.into())),
            );
        }
        Ok(answers)
    }
}

pub struct Temporary(pub PathBuf);
impl Temporary {
    pub fn new(label: &str) -> Self {
        Self(std::env::temp_dir().join(format!(
            "starter-{label}-{}-{}",
            std::process::id(),
            local::unique()
        )))
    }
}
impl Drop for Temporary {
    fn drop(&mut self) {
        if self.0.is_dir() {
            let _ = fs::remove_dir_all(&self.0);
        } else {
            let _ = fs::remove_file(&self.0);
        }
    }
}

pub fn exists(project: &Path) -> bool {
    project.join(".starterbase").exists()
}

pub fn apply(
    project: &Path,
    source: &Path,
    options: &SeedOptions,
    id: &str,
    revision: &str,
) -> Result<seed::State> {
    Ok(seed::apply(
        project,
        source,
        &options.answers()?,
        &options.reset,
        options.interactive(),
        id,
        revision,
    )?)
}

pub fn install(
    project: &Path,
    options: &SeedOptions,
    id: &str,
    revision: &str,
) -> Result<seed::State> {
    Ok(seed::install(
        project,
        &project.join(".starterbase"),
        &options.answers()?,
        options.interactive(),
        id,
        revision,
    )?)
}

pub fn seed(options: &SeedOptions, check: bool) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let root = if exists(&cwd) {
        cwd
    } else {
        local::repo_root().unwrap_or(cwd)
    };
    if !exists(&root) {
        if check {
            local::validate_checkout(&root)?;
        }
        println!("No .starterbase recipe; existing starter files are ready to use.");
        return Ok(());
    }
    if check {
        template::compile(&root.join(".starterbase"))?;
        println!("Recipe and templates are valid; no commands were executed.");
        return Ok(());
    }
    let binding = local::load_binding(&root).ok();
    let previous = seed::load_state(&root)?;
    let id = binding.as_ref().map(|b| b.id.as_str()).unwrap_or("local");
    let revision = if binding
        .as_ref()
        .is_some_and(|b| b.mode == local::Mode::Download)
    {
        previous
            .as_ref()
            .map(|s| s.revision.clone())
            .unwrap_or_else(|| "local".into())
    } else {
        local::head(&root).unwrap_or_else(|_| "local".into())
    };
    apply(&root, &root.join(".starterbase"), options, id, &revision)?;
    println!("Seeded {}", root.display());
    Ok(())
}

pub fn commit(project: &Path, message: &str) -> Result<()> {
    commit_history(project, message, false)
}

fn commit_history(project: &Path, message: &str, allow_empty: bool) -> Result<()> {
    let head = local::head(project)?;
    let index = PathBuf::from(local::run_git(
        project,
        &["rev-parse", "--git-path", "index"],
    )?);
    let index = if index.is_absolute() {
        index
    } else {
        project.join(index)
    };
    let saved_index = match fs::read(&index) {
        Ok(bytes) => Some((bytes, fs::metadata(&index)?.permissions())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    let markers = keep_files(project)?;
    let result = (|| -> Result<()> {
        local::stage(project)?;
        if allow_empty || !local::run_git(project, &["status", "--porcelain"])?.is_empty() {
            // This is the final fallible operation: once HEAD advances, apply is complete.
            local::run_git(
                project,
                &[
                    "-c",
                    "user.name=Starter",
                    "-c",
                    "user.email=starter@localhost",
                    "commit",
                    if allow_empty {
                        "--allow-empty"
                    } else {
                        "--no-allow-empty"
                    },
                    "-m",
                    message,
                ],
            )?;
        }
        Ok(())
    })();
    if let Err(failure) = result {
        let rollback = (|| -> Result<()> {
            for path in keep_files(project)?
                .keys()
                .filter(|path| !markers.contains_key(*path))
            {
                fs::remove_file(path)?;
            }
            for (path, (bytes, permissions)) in &markers {
                restore_file(path, bytes, permissions.clone())?;
            }
            match saved_index {
                Some((bytes, permissions)) => restore_file(&index, &bytes, permissions)?,
                None => match fs::remove_file(&index) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.into()),
                },
            }
            let current = local::head(project)?;
            if current != head {
                local::run_git(project, &["update-ref", "HEAD", &head, &current])?;
            }
            Ok(())
        })();
        return match rollback {
            Ok(()) => Err(failure),
            Err(error) => {
                Err(format!("{failure}; Git history rollback needs attention: {error}").into())
            }
        };
    }
    Ok(())
}

// local::stage creates/empties these files even in existing directories. Keep
// their prior bytes and permissions so a rejected Git hook leaves no artifacts.
fn keep_files(root: &Path) -> Result<BTreeMap<PathBuf, (Vec<u8>, fs::Permissions)>> {
    let mut files = BTreeMap::new();
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let name = entry.file_name();
        if name == ".git"
            || name == ".starter"
            || (name == ".state" && root.ends_with(".starterbase"))
        {
            continue;
        }
        let path = entry.path();
        let kind = entry.file_type()?;
        if name == ".siliconkeep" {
            if !kind.is_file() {
                return Err(format!("{} must be a regular file", path.display()).into());
            }
            files.insert(
                path.clone(),
                (fs::read(&path)?, entry.metadata()?.permissions()),
            );
        } else if kind.is_dir() {
            files.extend(keep_files(&path)?);
        }
    }
    Ok(files)
}

fn restore_file(path: &Path, bytes: &[u8], permissions: fs::Permissions) -> Result<()> {
    let temporary = path.with_extension(format!("restore-{}", local::unique()));
    let result = (|| -> Result<()> {
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(bytes)?;
        file.set_permissions(permissions)?;
        drop(file);
        fs::rename(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

pub fn update(project: &Path, archive: &Path, revision: &str, id: &str) -> Result<()> {
    let source = Temporary::new("recipe-source");
    local::clone_bundle(archive, &source.0, revision)?;
    if !exists(&source.0) {
        return Err(
            "updated starter removed .starterbase; keep the installed files and migrate explicitly"
                .into(),
        );
    }
    seed::apply_finalized(
        project,
        &source.0.join(".starterbase"),
        &Map::new(),
        &[],
        false,
        id,
        revision,
        |_| {
            commit_history(
                project,
                &format!("Update Starter recipe to {revision}"),
                true,
            )
            .map_err(|e| e.to_string())
        },
    )?;
    Ok(())
}

pub fn trim_install(project: &Path, state: &seed::State) -> Result<()> {
    // Recipe installations contain ingredients and declared/generated outputs only.
    let output = std::process::Command::new("git")
        .args(["ls-files", "-z"])
        .current_dir(project)
        .output()?;
    if !output.status.success() {
        return Err("cannot enumerate installed source files".into());
    }
    for bytes in output.stdout.split(|b| *b == 0).filter(|s| !s.is_empty()) {
        let name = std::str::from_utf8(bytes)?;
        if name.starts_with(".starterbase/") || state.generated.iter().any(|p| p == name) {
            continue;
        }
        let path = project.join(name);
        if path.is_file() || path.is_symlink() {
            fs::remove_file(path)?;
        }
    }
    Ok(())
}

pub fn prepare_push(project: &Path, id: &str) -> Result<()> {
    if !exists(project) {
        return Ok(());
    }
    template::compile(&project.join(".starterbase"))?;
    if !local::run_git(project, &["status", "--porcelain"])?.is_empty() {
        return Err("commit your changes before pushing; the default template preview is built from committed source".into());
    }
    let stage = Temporary::new("publish-preview");
    local::run_git(
        project,
        &[
            "clone",
            "--no-hardlinks",
            project.to_str().ok_or("project path is not UTF-8")?,
            stage.0.to_str().ok_or("temporary path is not UTF-8")?,
        ],
    )?;
    let previous = seed::load_state(project)?;
    copy_tree(
        &project.join(".starterbase/.state"),
        &stage.0.join(".starterbase/.state"),
    )?;
    seed::preview(&stage.0)?;
    commit(&stage.0, "Build default Starter preview")?;
    local::validate_checkout(&stage.0)?;
    local::run_git(
        project,
        &[
            "fetch",
            stage.0.to_str().ok_or("temporary path is not UTF-8")?,
            "main",
        ],
    )?;
    local::run_git(project, &["merge", "--ff-only", "FETCH_HEAD"])?;
    // Preserve personal answers for the next seed, with the newly installed
    // default preview as its pure generated baseline.
    let state_dir = Path::new(".starterbase/.state");
    copy_tree(&stage.0.join(state_dir), &project.join(state_dir))?;
    {
        let path = project.join(state_dir).join("state.json");
        let mut state: Value = serde_json::from_slice(&fs::read(&path)?)?;
        if let Some(previous) = previous {
            state["answers"] = Value::Object(previous.answers);
        }
        state["source_id"] = Value::String(id.into());
        state["revision"] = Value::String(local::head(project)?);
        local::atomic_write(&path, &serde_json::to_vec_pretty(&state)?)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        }
    }
    Ok(())
}

fn copy_tree(source: &Path, target: &Path) -> Result<()> {
    if !source.exists() {
        return Ok(());
    }
    if target.exists() {
        fs::remove_dir_all(target)?;
    }
    fs::create_dir_all(target)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(target, fs::Permissions::from_mode(0o700))?;
    }
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &target.join(entry.file_name()))?;
        } else {
            fs::copy(entry.path(), target.join(entry.file_name()))?;
        }
    }
    Ok(())
}
