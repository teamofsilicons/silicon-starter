//! Instance builds: merge pure generated output, then apply files and answers together.
use crate::template;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct State {
    pub source_id: String,
    pub revision: String,
    pub answers: Map<String, Value>,
    pub auto_update: bool,
    #[serde(default)]
    pub generated: Vec<String>,
}

pub fn load_state(project: &Path) -> Result<Option<State>, String> {
    let path = project.join(".starterbase/.state/state.json");
    match fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| format!("invalid instance state at {}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("cannot read {}: {e}", path.display())),
    }
}

pub fn auto_update(project: &Path) -> Result<bool, String> {
    Ok(template::compile(&project.join(".starterbase"))?.auto_update)
}

pub fn set_auto_update(project: &Path, enabled: bool) -> Result<(), String> {
    let _lock = lock_project(project)?;
    template::set_auto_update(&project.join(".starterbase"), enabled)
}

/// Build/reconfigure an instance or install a staged upstream recipe.
/// `source` contains starter.yaml, and may be the installed .starterbase itself.
pub fn apply(
    project: &Path,
    source: &Path,
    explicit: &Map<String, Value>,
    reset: &[String],
    interactive: bool,
    source_id: &str,
    revision: &str,
) -> Result<State, String> {
    apply_finalized(
        project,
        source,
        explicit,
        reset,
        interactive,
        source_id,
        revision,
        |_| Ok(()),
    )
}

/// Apply the build and finalize caller-owned bookkeeping before releasing the
/// instance lock. A failed finalizer restores every generated/source/state file;
/// the finalizer must restore its own side effects before returning an error.
#[allow(clippy::too_many_arguments)]
pub fn apply_finalized(
    project: &Path,
    source: &Path,
    explicit: &Map<String, Value>,
    reset: &[String],
    interactive: bool,
    source_id: &str,
    revision: &str,
    finalize: impl FnOnce(&State) -> Result<(), String>,
) -> Result<State, String> {
    apply_inner(
        project,
        source,
        explicit,
        reset,
        interactive,
        source_id,
        revision,
        false,
        finalize,
    )
}

/// Configure a freshly downloaded, untouched published preview. The caller must
/// guarantee this is a new checkout: its preview files are the initial baseline.
pub fn install(
    project: &Path,
    source: &Path,
    explicit: &Map<String, Value>,
    interactive: bool,
    source_id: &str,
    revision: &str,
) -> Result<State, String> {
    apply_inner(
        project,
        source,
        explicit,
        &[],
        interactive,
        source_id,
        revision,
        true,
        |_| Ok(()),
    )
}

#[allow(clippy::too_many_arguments)]
fn apply_inner(
    project: &Path,
    source: &Path,
    explicit: &Map<String, Value>,
    reset: &[String],
    interactive: bool,
    source_id: &str,
    revision: &str,
    fresh_install: bool,
    finalize: impl FnOnce(&State) -> Result<(), String>,
) -> Result<State, String> {
    let project = std::path::absolute(project).map_err(err)?;
    let _lock = lock_project(&project)?;
    let old_state = if fresh_install {
        None
    } else {
        load_state(&project)?
    };
    let mut saved = old_state
        .as_ref()
        .map(|s| s.answers.clone())
        .unwrap_or_default();
    for name in reset {
        saved.remove(name);
    }
    let scratch = Scratch::new(&project)?;
    let staged_source = scratch.0.join("source");
    copy_source(source, &staged_source)?;
    if project.join(".starterbase/starter.yaml").is_file() {
        template::set_auto_update(&staged_source, auto_update(&project)?)?;
    }
    ensure_ignore(&staged_source)?;
    if !reset.is_empty() {
        let recipe = template::compile(&staged_source)?;
        for name in reset {
            if !recipe.variables.contains_key(name) {
                return Err(format!("unknown reset variable `{name}`"));
            }
        }
    }
    let baseline = if fresh_install {
        None
    } else if old_state.is_some() {
        let path = project.join(".starterbase/.state/generated");
        if !path.join("silicon.yaml").is_file() {
            return Err(
                "missing generated baseline; restore .starterbase/.state before updating".into(),
            );
        }
        Some(path)
    } else if project.join("silicon.yaml").exists() {
        // A fresh clone has the published default preview, but no private state.
        // Recover only if that preview still matches; never guess lost answers.
        let source = project.join(".starterbase");
        if !source.join("starter.yaml").is_file() {
            return Err(
                "missing generated baseline; reconcile existing files before seeding".into(),
            );
        }
        let path = scratch.0.join("recovered");
        template::build(&source, &project, &path, &Map::new(), &Map::new(), false)?;
        for (name, expected) in inventory(&path)?.files {
            if live_file(&project, &name)?.as_ref().map(|f| &f.bytes) != Some(&expected.bytes) {
                return Err(format!(
                    "missing generated baseline: {} differs from the published defaults; restore .starterbase/.state or reconcile the file",
                    name.display()
                ));
            }
        }
        Some(path)
    } else {
        None
    };
    let output = scratch.0.join("output");
    let built = template::build(
        &staged_source,
        &project,
        &output,
        explicit,
        &saved,
        interactive,
    )?;
    let new = inventory(&output)?;
    let old = if fresh_install {
        let mut published = Tree::default();
        for path in new.files.keys() {
            if let Some(file) = live_file(&project, path)? {
                published.files.insert(path.clone(), file);
            }
        }
        published
    } else {
        match baseline {
            Some(path) => inventory(&path)?,
            None => Tree::default(),
        }
    };
    if let Some(previous) = &old_state {
        let expected: BTreeSet<_> = previous.generated.iter().map(PathBuf::from).collect();
        let actual: BTreeSet<_> = old.dirs.iter().chain(old.files.keys()).cloned().collect();
        if !expected.is_empty() && expected != actual {
            return Err(
                "incomplete generated baseline; restore .starterbase/.state before updating".into(),
            );
        }
    }
    let (merged, observed) = merge(&project, &old, &new, &scratch.0)?;
    let merged_dir = scratch.0.join("merged");
    write_tree(&merged_dir, &merged)?;
    let removed: Vec<String> = old
        .files
        .keys()
        .chain(new.files.keys())
        .filter(|path| !merged.files.contains_key(*path))
        .map(|path| path.to_string_lossy().into_owned())
        .collect();
    template::validate_merged_output(&merged_dir, &project, &removed)?;
    let state = State {
        source_id: source_id.into(),
        revision: revision.into(),
        answers: built.answers,
        auto_update: built.auto_update,
        generated: new
            .dirs
            .iter()
            .chain(new.files.keys())
            .map(|p| p.to_string_lossy().into_owned())
            .collect(),
    };
    let state_dir = staged_source.join(".state");
    private_dir(&state_dir)?;
    write_tree(&state_dir.join("generated"), &new)?;
    private_write(
        &state_dir.join("state.json"),
        &serde_json::to_vec_pretty(&state).map_err(err)?,
        false,
    )?;
    commit_finalized(
        &project,
        &old,
        &new,
        &merged,
        Some(&staged_source),
        Some(&observed),
        &scratch.0,
        || finalize(&state),
    )?;
    Ok(state)
}

/// Publish a default preview in a disposable checkout, never using local answers.
pub fn preview(project: &Path) -> Result<State, String> {
    let project = std::path::absolute(project).map_err(err)?;
    let _lock = lock_project(&project)?;
    let scratch = Scratch::new(&project)?;
    let output = scratch.0.join("output");
    let source = scratch.0.join("source");
    copy_source(&project.join(".starterbase"), &source)?;
    ensure_ignore(&source)?;
    let built = template::build(&source, &project, &output, &Map::new(), &Map::new(), false)?;
    let new = inventory(&output)?;
    let previous = load_state(&project)?;
    let previous_generated = project.join(".starterbase/.state/generated");
    let old = if previous.is_some() && previous_generated.is_dir() {
        inventory(&previous_generated)?
    } else {
        Tree::default()
    };
    let state = State {
        source_id: previous
            .as_ref()
            .map(|s| s.source_id.clone())
            .unwrap_or_default(),
        revision: previous
            .as_ref()
            .map(|s| s.revision.clone())
            .unwrap_or_default(),
        answers: built.answers,
        auto_update: built.auto_update,
        generated: new
            .dirs
            .iter()
            .chain(new.files.keys())
            .map(|p| p.to_string_lossy().into_owned())
            .collect(),
    };
    let state_dir = source.join(".state");
    private_dir(&state_dir)?;
    write_tree(&state_dir.join("generated"), &new)?;
    private_write(
        &state_dir.join("state.json"),
        &serde_json::to_vec_pretty(&state).map_err(err)?,
        false,
    )?;
    commit(&project, &old, &new, &new, Some(&source), None, &scratch.0)?;
    Ok(state)
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct File {
    bytes: Vec<u8>,
    executable: bool,
}
#[derive(Default)]
struct Tree {
    files: BTreeMap<PathBuf, File>,
    dirs: BTreeSet<PathBuf>,
}

fn inventory(root: &Path) -> Result<Tree, String> {
    fn walk(root: &Path, relative: &Path, tree: &mut Tree) -> Result<(), String> {
        for entry in fs::read_dir(root.join(relative)).map_err(err)? {
            let entry = entry.map_err(err)?;
            let path = relative.join(entry.file_name());
            let metadata = entry.file_type().map_err(err)?;
            if relative.as_os_str().is_empty()
                && [".git", ".starterbase"].iter().any(|name| {
                    entry
                        .file_name()
                        .to_string_lossy()
                        .eq_ignore_ascii_case(name)
                })
            {
                return Err(format!(
                    "generated output uses reserved path {}",
                    path.display()
                ));
            }
            if metadata.is_dir() {
                tree.dirs.insert(path.clone());
                walk(root, &path, tree)?;
            } else if metadata.is_file() {
                tree.files.insert(path, read_file(&entry.path())?);
            } else {
                return Err(format!(
                    "generated output must contain regular files and directories: {}",
                    path.display()
                ));
            }
        }
        Ok(())
    }
    let mut tree = Tree::default();
    walk(root, Path::new(""), &mut tree)?;
    Ok(tree)
}

fn read_file(path: &Path) -> Result<File, String> {
    #[cfg(unix)]
    let executable = {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path).map_err(err)?.permissions().mode() & 0o111 != 0
    };
    #[cfg(not(unix))]
    let executable = false;
    Ok(File {
        bytes: fs::read(path).map_err(err)?,
        executable,
    })
}

fn safe_path(project: &Path, relative: &Path) -> Result<PathBuf, String> {
    let mut path = project.to_path_buf();
    for part in relative.components() {
        if !matches!(part, std::path::Component::Normal(_)) {
            return Err(format!("unsafe generated path {}", relative.display()));
        }
        path.push(part);
        match fs::symlink_metadata(&path) {
            Ok(m) if m.file_type().is_symlink() => {
                return Err(format!(
                    "generated path collides with a symlink: {}",
                    path.display()
                ));
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("cannot inspect {}: {e}", path.display())),
        }
    }
    Ok(path)
}

fn live_file(project: &Path, relative: &Path) -> Result<Option<File>, String> {
    let path = safe_path(project, relative)?;
    match fs::symlink_metadata(&path) {
        Ok(m) if m.is_file() => read_file(&path).map(Some),
        Ok(_) => Err(format!(
            "generated file collides with a directory: {}",
            relative.display()
        )),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(err(e)),
    }
}

type Observed = BTreeMap<PathBuf, Option<File>>;
fn merge(
    project: &Path,
    old: &Tree,
    new: &Tree,
    scratch: &Path,
) -> Result<(Tree, Observed), String> {
    let mut observed = BTreeMap::new();
    let mut merged = Tree {
        files: BTreeMap::new(),
        dirs: new.dirs.clone(),
    };
    let paths: BTreeSet<_> = old.files.keys().chain(new.files.keys()).collect();
    for path in paths {
        let local = live_file(project, path)?;
        observed.insert(path.clone(), local.clone());
        let base = old.files.get(path);
        let next = new.files.get(path);
        let chosen = if local.as_ref() == base {
            next.cloned()
        } else if next == base || local.as_ref() == next {
            local.clone()
        } else if let (Some(local), Some(base), Some(next)) = (&local, base, next) {
            Some(merge_file(local, base, next, path, scratch)?)
        } else {
            return Err(format!(
                "generated file conflict: {}; reconcile the local file and retry",
                path.display()
            ));
        };
        if let Some(file) = chosen {
            merged.files.insert(path.clone(), file);
        }
    }
    for dir in &merged.dirs {
        let path = safe_path(project, dir)?;
        if path.exists() && !path.is_dir() {
            return Err(format!(
                "generated directory collides with a local file: {}",
                dir.display()
            ));
        }
    }
    Ok((merged, observed))
}

fn merge_file(
    local: &File,
    base: &File,
    next: &File,
    path: &Path,
    scratch: &Path,
) -> Result<File, String> {
    let inputs = [
        scratch.join("local"),
        scratch.join("base"),
        scratch.join("next"),
    ];
    for (path, file) in inputs.iter().zip([local, base, next]) {
        private_write(path, &file.bytes, file.executable)?;
    }
    let result = Command::new("git")
        .args(["merge-file", "-p", "--diff3"])
        .args(&inputs)
        .output()
        .map_err(|e| format!("cannot execute git merge-file: {e}"))?;
    if !result.status.success() {
        return Err(format!(
            "generated file conflict: {}; update left unapplied",
            path.display()
        ));
    }
    Ok(File {
        bytes: result.stdout,
        executable: if local.executable == base.executable {
            next.executable
        } else {
            local.executable
        },
    })
}

fn write_tree(root: &Path, tree: &Tree) -> Result<(), String> {
    private_dir(root)?;
    for dir in &tree.dirs {
        private_dir(&root.join(dir))?;
    }
    for (path, file) in &tree.files {
        if let Some(parent) = root.join(path).parent() {
            private_dir(parent)?;
        }
        private_write(&root.join(path), &file.bytes, file.executable)?;
    }
    Ok(())
}

// All merges and validation finish before touching live files. Renamed originals
// provide rollback if any filesystem operation fails during installation.
fn commit(
    project: &Path,
    old: &Tree,
    new: &Tree,
    merged: &Tree,
    source: Option<&Path>,
    observed: Option<&Observed>,
    scratch: &Path,
) -> Result<(), String> {
    commit_finalized(project, old, new, merged, source, observed, scratch, || {
        Ok(())
    })
}

#[allow(clippy::too_many_arguments)]
fn commit_finalized(
    project: &Path,
    old: &Tree,
    new: &Tree,
    merged: &Tree,
    source: Option<&Path>,
    observed: Option<&Observed>,
    scratch: &Path,
    finalize: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    if let Some(observed) = observed {
        for (path, expected) in observed {
            if live_file(project, path)?.as_ref() != expected.as_ref() {
                return Err(format!(
                    "{} changed during the build; retry the update",
                    path.display()
                ));
            }
        }
    }
    fs::create_dir_all(project).map_err(err)?;
    let backup = scratch.join("backup");
    private_dir(&backup)?;
    let paths: BTreeSet<_> = old.files.keys().chain(new.files.keys()).cloned().collect();
    let mut originals = Vec::new();
    let mut installed = Vec::new();
    let mut created_dirs = Vec::new();
    let result = (|| {
        for path in &paths {
            let target = safe_path(project, path)?;
            let current = live_file(project, path)?;
            if let Some(observed) = observed
                && current.as_ref() != observed.get(path).and_then(Option::as_ref)
            {
                return Err(format!(
                    "{} changed during the apply; retry the update",
                    path.display()
                ));
            }
            if let Some(current) = current {
                if let Some(next) = merged.files.get(path)
                    && current.bytes == next.bytes
                    && current.executable == next.executable
                {
                    continue;
                }
                let saved = backup.join(path);
                fs::create_dir_all(saved.parent().unwrap()).map_err(err)?;
                fs::rename(&target, &saved).map_err(err)?;
                originals.push(path.clone());
            }
            if let Some(file) = merged.files.get(path) {
                create_dirs(
                    project,
                    path.parent().unwrap_or(Path::new("")),
                    &mut created_dirs,
                )?;
                installed.push(path.clone());
                private_write(&target, &file.bytes, file.executable)?;
            }
        }
        for dir in &new.dirs {
            create_dirs(project, dir, &mut created_dirs)?;
        }
        if let Some(source) = source {
            let target = safe_path(project, Path::new(".starterbase"))?;
            if target.exists() {
                fs::rename(&target, backup.join(".starterbase")).map_err(err)?;
                originals.push(PathBuf::from(".starterbase"));
            }
            fs::rename(source, &target).map_err(err)?;
            installed.push(PathBuf::from(".starterbase"));
        }
        finalize()
    })();
    if let Err(failure) = result {
        let mut rollback_errors = Vec::new();
        for path in installed.iter().rev() {
            let target = project.join(path);
            let removal = if target.is_dir() {
                fs::remove_dir_all(target)
            } else {
                fs::remove_file(target)
            };
            if let Err(e) = removal
                && e.kind() != std::io::ErrorKind::NotFound
            {
                rollback_errors.push(e.to_string());
            }
        }
        for path in originals.iter().rev() {
            if let Err(e) = fs::rename(backup.join(path), project.join(path)) {
                rollback_errors.push(e.to_string());
            }
        }
        for dir in created_dirs.iter().rev() {
            let _ = fs::remove_dir(project.join(dir));
        }
        if !rollback_errors.is_empty() {
            // Keep backups recoverable; Scratch must not discard the originals.
            let _ = fs::write(scratch.join(".keep"), b"rollback needs attention");
            return Err(format!(
                "{failure}; rollback needs attention at {}: {}",
                backup.display(),
                rollback_errors.join("; ")
            ));
        }
        return Err(failure);
    }
    Ok(())
}

fn create_dirs(project: &Path, path: &Path, created: &mut Vec<PathBuf>) -> Result<(), String> {
    let mut relative = PathBuf::new();
    for part in path.components() {
        relative.push(part);
        let target = safe_path(project, &relative)?;
        if !target.exists() {
            fs::create_dir(&target).map_err(err)?;
            created.push(relative.clone());
        } else if !target.is_dir() {
            return Err(format!(
                "generated directory collides with a file: {}",
                relative.display()
            ));
        }
    }
    Ok(())
}

fn copy_source(from: &Path, to: &Path) -> Result<(), String> {
    private_dir(to)?;
    for entry in fs::read_dir(from).map_err(err)? {
        let entry = entry.map_err(err)?;
        if entry.file_name() == ".state" {
            continue;
        }
        let target = to.join(entry.file_name());
        let kind = entry.file_type().map_err(err)?;
        if kind.is_dir() {
            copy_source(&entry.path(), &target)?;
        } else if kind.is_file() {
            let file = read_file(&entry.path())?;
            private_write(&target, &file.bytes, file.executable)?;
        } else {
            return Err(format!(
                "recipe ingredients cannot be symlinks: {}",
                entry.path().display()
            ));
        }
    }
    Ok(())
}

fn ensure_ignore(source: &Path) -> Result<(), String> {
    let path = source.join(".gitignore");
    let mut text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(err(e)),
    };
    if text.lines().last() != Some("/.state/") {
        if !text.ends_with('\n') && !text.is_empty() {
            text.push('\n');
        }
        text.push_str("/.state/\n");
        private_write(&path, text.as_bytes(), false)?;
    }
    Ok(())
}

struct ProjectLock(fs::File);
impl Drop for ProjectLock {
    fn drop(&mut self) {
        // A concurrently spawned child can briefly inherit this file description.
        let _ = self.0.unlock();
    }
}

fn lock_project(project: &Path) -> Result<ProjectLock, String> {
    let project = std::path::absolute(project).map_err(err)?;
    let parent = project
        .parent()
        .ok_or("project requires a parent directory")?;
    fs::create_dir_all(parent).map_err(err)?;
    let project = if project.exists() {
        project.canonicalize().map_err(err)?
    } else {
        parent.canonicalize().map_err(err)?.join(
            project
                .file_name()
                .ok_or("project requires a directory name")?,
        )
    };
    let lock_path = if project.join(".git").is_dir() {
        project.join(".git/starter-seed.lock")
    } else {
        project.parent().unwrap().join(format!(
            ".starter-seed-{}.lock",
            project
                .file_name()
                .ok_or("project requires a directory name")?
                .to_string_lossy()
        ))
    };
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(lock_path).map_err(err)?;
    file.try_lock()
        .map_err(|e| format!("another Starter operation is using this project: {e}"))?;
    Ok(ProjectLock(file))
}

fn private_dir(path: &Path) -> Result<(), String> {
    fs::create_dir_all(path).map_err(err)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(err)?;
    }
    Ok(())
}
fn private_write(path: &Path, bytes: &[u8], executable: bool) -> Result<(), String> {
    use std::io::Write;
    #[cfg(not(unix))]
    let _ = executable;
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(if executable { 0o700 } else { 0o600 });
    }
    let mut file = options.open(path).map_err(err)?;
    file.write_all(bytes).map_err(err)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(if executable {
            0o700
        } else {
            0o600
        }))
        .map_err(err)?;
    }
    Ok(())
}
fn err(error: impl std::fmt::Display) -> String {
    error.to_string()
}
struct Scratch(PathBuf);
impl Scratch {
    fn new(project: &Path) -> Result<Self, String> {
        let parent = project
            .parent()
            .ok_or("project requires a parent directory")?;
        fs::create_dir_all(parent).map_err(err)?;
        let path = parent.join(format!(".starter-build-{}", uuid::Uuid::new_v4()));
        private_dir(&path)?;
        Ok(Self(path))
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        if !self.0.join(".keep").exists() {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn three_way_merge_preserves_edits_and_defers_conflicts_and_deletions() {
        let root = std::env::temp_dir().join(format!("starter-seed-test-{}", uuid::Uuid::new_v4()));
        let project = root.join("project");
        fs::create_dir_all(&project).unwrap();
        let scratch = Scratch::new(&project).unwrap();
        let held_lock = lock_project(&project).unwrap();
        assert!(lock_project(&project).is_err());
        let inherited = held_lock.0.try_clone().unwrap();
        drop(held_lock);
        drop(lock_project(&project).unwrap());
        drop(inherited);
        let file = |text: &str| File {
            bytes: text.as_bytes().to_vec(),
            executable: false,
        };
        let path = PathBuf::from("tools.md");
        let mut old = Tree::default();
        old.files
            .insert(path.clone(), file("one\ntwo\nthree\nfour\nfive\n"));
        let mut new = Tree::default();
        new.files
            .insert(path.clone(), file("one\ntwo\nthree\nfour\ncreator\n"));
        fs::write(project.join(&path), "local\ntwo\nthree\nfour\nfive\n").unwrap();
        let (merged, observed) = merge(&project, &old, &new, &scratch.0).unwrap();
        assert_eq!(
            merged.files[&path].bytes,
            b"local\ntwo\nthree\nfour\ncreator\n"
        );
        fs::write(project.join(&path), "concurrent edit\n").unwrap();
        assert!(
            commit(
                &project,
                &old,
                &new,
                &merged,
                None,
                Some(&observed),
                &scratch.0
            )
            .is_err()
        );
        assert_eq!(
            fs::read_to_string(project.join(&path)).unwrap(),
            "concurrent edit\n"
        );
        fs::write(project.join(&path), "local\ntwo\nthree\nfour\nfive\n").unwrap();
        commit(
            &project,
            &old,
            &new,
            &merged,
            None,
            Some(&observed),
            &scratch.0,
        )
        .unwrap();
        assert_eq!(
            fs::read(project.join(&path)).unwrap(),
            merged.files[&path].bytes
        );
        assert_eq!(new.files[&path].bytes, b"one\ntwo\nthree\nfour\ncreator\n");
        new.files
            .insert(path.clone(), file("creator\ntwo\nthree\nfour\nfive\n"));
        assert!(merge(&project, &old, &new, &scratch.0).is_err());
        assert!(merge(&project, &old, &Tree::default(), &scratch.0).is_err());
        assert!(
            !fs::read_to_string(project.join(&path))
                .unwrap()
                .contains("<<<<<<<")
        );
        // A late I/O/path failure restores earlier files and removes additions.
        let before = fs::read(project.join(&path)).unwrap();
        let mut invalid = Tree::default();
        invalid
            .files
            .insert(PathBuf::from("added.md"), file("temporary"));
        invalid
            .files
            .insert(path.clone(), file("temporary replacement"));
        invalid.dirs.insert(path.join("impossible"));
        assert!(commit(&project, &new, &invalid, &invalid, None, None, &scratch.0).is_err());
        assert_eq!(fs::read(project.join(&path)).unwrap(), before);
        assert!(!project.join("added.md").exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn sample_install_and_update_keep_answers_pure_baseline_and_transaction() {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("project");
        let source =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../starter_template/.starterbase");
        if !source.exists() {
            return;
        }
        let source = source.canonicalize().unwrap();
        let installed_source = project.join(".starterbase");
        copy_source(&source, &installed_source).unwrap();
        let publisher_answers = serde_json::json!({"timezone":"Publisher/Timezone"})
            .as_object()
            .unwrap()
            .clone();
        let published = root.path().join("published");
        template::build(
            &source,
            &project,
            &published,
            &publisher_answers,
            &Map::new(),
            false,
        )
        .unwrap();
        write_tree(&project, &inventory(&published).unwrap()).unwrap();
        let answers = serde_json::json!({"timezone":"Installer/Timezone", "silicon_id":"research:tos", "silicon_token":"private-token"}).as_object().unwrap().clone();
        let state = install(
            &project,
            &installed_source,
            &answers,
            false,
            "sample",
            "revision-one",
        )
        .unwrap();
        assert_eq!(state.answers["timezone"], "Installer/Timezone");
        assert!(
            fs::read_to_string(project.join("silicon.yaml"))
                .unwrap()
                .contains("private-token")
        );
        assert!(project.join("workspace").is_dir());
        fs::write(project.join("workspace/personal.txt"), "user content").unwrap();
        set_auto_update(&project, false).unwrap();
        let tools = project.join("prompts/tools.md");
        let local_tools = fs::read_to_string(&tools)
            .unwrap()
            .replace("# Tools", "# Team tools");
        fs::write(&tools, &local_tools).unwrap();
        let upgrade = root.path().join("upgrade");
        copy_source(&source, &upgrade).unwrap();
        let fragment = upgrade.join("optional/waveform.md");
        let creator_text = fs::read_to_string(&fragment).unwrap().replace(
            "share generated audio where a tool expects a file link.",
            "share generated audio links with the team.",
        );
        fs::write(&fragment, &creator_text).unwrap();
        let state = apply(
            &project,
            &upgrade,
            &Map::new(),
            &[],
            false,
            "sample",
            "revision-two",
        )
        .unwrap();
        assert_eq!(state.revision, "revision-two");
        assert_eq!(state.answers["silicon_token"], "private-token");
        assert!(!state.auto_update);
        let merged = fs::read_to_string(&tools).unwrap();
        assert!(merged.contains("# Team tools"));
        assert!(merged.contains("share generated audio links with the team."));
        let baseline =
            fs::read_to_string(project.join(".starterbase/.state/generated/prompts/tools.md"))
                .unwrap();
        assert!(baseline.starts_with("# Tools"));
        assert!(!baseline.contains("# Team tools"));
        assert_eq!(
            fs::read_to_string(project.join("workspace/personal.txt")).unwrap(),
            "user content"
        );
        let recipe = fs::read(project.join(".starterbase/starter.yaml")).unwrap();
        let saved = fs::read(project.join(".starterbase/.state/state.json")).unwrap();
        let prompt_before = fs::read(project.join("prompts/silicon.md")).unwrap();
        let explicit = serde_json::json!({"purpose":"new purpose"})
            .as_object()
            .unwrap()
            .clone();
        let rejected = apply_finalized(
            &project,
            &upgrade,
            &explicit,
            &[],
            false,
            "sample",
            "rejected-revision",
            |state| {
                assert_eq!(state.revision, "rejected-revision");
                assert!(lock_project(&project).is_err());
                Err("history commit rejected".into())
            },
        );
        assert_eq!(rejected.unwrap_err(), "history commit rejected");
        assert_eq!(
            fs::read(project.join("prompts/silicon.md")).unwrap(),
            prompt_before
        );
        assert_eq!(
            fs::read(project.join(".starterbase/starter.yaml")).unwrap(),
            recipe
        );
        assert_eq!(
            fs::read(project.join(".starterbase/.state/state.json")).unwrap(),
            saved
        );
        assert_eq!(
            fs::read_to_string(project.join(".starterbase/.state/generated/prompts/tools.md"))
                .unwrap(),
            baseline
        );
        let template = upgrade.join("prompts/tools.md.tmpl");
        let changed = fs::read_to_string(&template)
            .unwrap()
            .replace("# Tools", "# Creator tools");
        fs::write(&template, changed).unwrap();
        let answer_change = serde_json::json!({"purpose":"new purpose"})
            .as_object()
            .unwrap()
            .clone();
        assert!(
            apply(
                &project,
                &upgrade,
                &answer_change,
                &[],
                false,
                "sample",
                "revision-three"
            )
            .is_err()
        );
        assert_eq!(fs::read_to_string(&tools).unwrap(), merged);
        assert_eq!(
            fs::read(project.join(".starterbase/starter.yaml")).unwrap(),
            recipe
        );
        assert_eq!(
            fs::read(project.join(".starterbase/.state/state.json")).unwrap(),
            saved
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(project.join(".starterbase/.state/state.json"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
            assert_eq!(
                fs::metadata(project.join(".starterbase/.state"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }
        let obsolete = "old-generated.txt";
        fs::write(project.join(obsolete), "old output").unwrap();
        fs::write(
            project.join(".starterbase/.state/generated").join(obsolete),
            "old output",
        )
        .unwrap();
        let defaults = preview(&project).unwrap();
        assert_eq!(defaults.answers["silicon_token"], "");
        assert!(
            !fs::read_to_string(project.join("silicon.yaml"))
                .unwrap()
                .contains("private-token")
        );
        assert!(!project.join(obsolete).exists());
        assert_eq!(
            fs::read_to_string(project.join("workspace/personal.txt")).unwrap(),
            "user content"
        );
        crate::local::run_git(&project, &["init", "-b", "main"]).unwrap();
        assert_eq!(
            crate::local::run_git(
                &project,
                &["check-ignore", ".starterbase/.state/state.json"]
            )
            .unwrap(),
            ".starterbase/.state/state.json"
        );
    }
}
