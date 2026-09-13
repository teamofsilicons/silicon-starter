use base64::{Engine, engine::general_purpose::STANDARD as B64};
use clap::{Parser, Subcommand};
use reqwest::Method;
use serde_json::{Value, json};
use silicon_starter_core::{
    SEED_YAML,
    local::{self, Binding, Mode},
    validate_silicon_yaml,
};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command as Process,
};

#[derive(Parser)]
#[command(
    name = "starter",
    version,
    about = "Version controlled silicon starter registry. Use --help to explore the command tree."
)]
struct Cli {
    #[arg(long, env = "STARTER_API_URL", default_value = "http://127.0.0.1:8080")]
    api: String,
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    Iam {
        #[arg(long)]
        json: bool,
    },
    Login {
        token: Option<String>,
        #[command(subcommand)]
        command: Option<LoginCommand>,
    },
    Init,
    New {
        visibility: String,
        id: String,
    },
    Commit {
        message: Option<String>,
        #[command(subcommand)]
        command: Option<CommitCommand>,
    },
    Push {
        id: Option<String>,
    },
    Pull {
        id: Option<String>,
    },
    Download {
        id: String,
    },
    Publish {
        selector: Option<String>,
        version: Option<String>,
        #[arg(long)]
        notes: Option<String>,
    },
    Update {
        mode: String,
    },
    Revert {
        target: String,
    },
    History {
        kind: Option<String>,
    },
    Search {
        q: String,
    },
    Show {
        id: String,
    },
    Star {
        id: String,
    },
    Fork {
        id: String,
    },
    Discussions {
        id: String,
    },
    Discuss {
        id: String,
        body: String,
        #[arg(long)]
        parent: Option<String>,
    },
    Report {
        body: String,
        #[arg(long)]
        pr: Option<String>,
    },
    Daemon {
        #[arg(long)]
        once: bool,
    },
    Webhook {
        url: String,
        #[arg(long)]
        secret: Option<String>,
    },
    Unhook,
    List,
}
#[derive(Subcommand)]
enum LoginCommand {
    Status {
        #[arg(long)]
        json: bool,
    },
}
#[derive(Subcommand)]
enum CommitCommand {
    History {
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let c = Cli::parse();
    match c.command {
        Command::Iam { json: true } => println!(
            "{}",
            serde_json::to_string_pretty(
                &json!({"app_id":"tos>starter","base_url":c.api,"docs":"https://starter.teamofsilicons.com/docs","source":"https://github.com/teamofsilicons/silicon-starter","package":"https://crates.io/crates/silicon-starter-core"})
            )?
        ),
        Command::Iam { json: false } => println!(
            "tos>starter\nAPI: {}\nDocs: https://starter.teamofsilicons.com/docs",
            c.api
        ),
        Command::Login {
            token: Some(token),
            command: None,
        } => login(&c.api, &token).await?,
        Command::Login {
            token: None,
            command: Some(LoginCommand::Status { json }),
        } => login_status(&c.api, json).await?,
        Command::Login { .. } => {
            return Err("usage: starter login <IAM SLT> | starter login status --json".into());
        }
        Command::Init => init()?,
        Command::New { visibility, id } => new_starter(&c.api, &visibility, &id).await?,
        Command::Commit {
            message: Some(m),
            command: None,
        } => {
            let root = local::repo_root()?;
            println!("{}", local::stage_and_commit(root, m.trim())?);
        }
        Command::Commit {
            message: None,
            command: Some(CommitCommand::History { limit }),
        } => println!("{}", local::commit_history(local::repo_root()?, limit)?),
        Command::Commit { .. } => {
            return Err("usage: starter commit \"message\" | starter commit history".into());
        }
        Command::Push { id } => push(&c.api, id.as_deref()).await?,
        Command::Pull { id } => pull(&c.api, id.as_deref(), false).await?,
        Command::Download { id } => pull(&c.api, Some(&id), true).await?,
        Command::Publish {
            selector,
            version,
            notes,
        } => {
            if selector.as_deref() == Some("history") {
                let b = local::load_binding(".")?;
                request(
                    &c.api,
                    &format!("/api/v1/starters/{}/versions", b.id),
                    Method::GET,
                    None,
                )
                .await?
            } else {
                publish(
                    &c.api,
                    &selector.ok_or("publish requires latest, a commit prefix, or history")?,
                    &version.ok_or("publish requires version Y.X")?,
                    notes.as_deref(),
                )
                .await?;
            }
        }
        Command::Update { mode } => update(&c.api, &mode).await?,
        Command::Revert { target } => revert(&target)?,
        Command::History { kind } => {
            let root = local::repo_root()?;
            match kind.as_deref().unwrap_or("commit") {
                "commit" => println!("{}", local::commit_history(root, 100)?),
                "publish" => {
                    let b = local::load_binding(".")?;
                    request(
                        &c.api,
                        &format!("/api/v1/starters/{}/versions", b.id),
                        Method::GET,
                        None,
                    )
                    .await?
                }
                x => {
                    return Err(
                        format!("unknown history kind `{x}`; expected commit or publish").into(),
                    );
                }
            }
        }
        Command::Search { q } => {
            request(
                &c.api,
                &format!("/api/v1/search?q={}", encode(&q)),
                Method::GET,
                None,
            )
            .await?
        }
        Command::Show { id } => {
            request(&c.api, &format!("/api/v1/starters/{id}"), Method::GET, None).await?
        }
        Command::Star { id } => {
            authed_request(
                &c.api,
                &format!("/api/v1/starters/{id}/star"),
                Method::POST,
                None,
            )
            .await?
        }
        Command::Fork { id } => {
            authed_request(
                &c.api,
                &format!("/api/v1/starters/{id}/fork"),
                Method::POST,
                None,
            )
            .await?
        }
        Command::Discussions { id } => {
            request(
                &c.api,
                &format!("/api/v1/starters/{id}/discussions"),
                Method::GET,
                None,
            )
            .await?
        }
        Command::Discuss { id, body, parent } => {
            authed_request(
                &c.api,
                &format!("/api/v1/starters/{id}/discussions"),
                Method::POST,
                Some(json!({"body":body,"parent_id":parent})),
            )
            .await?
        }
        Command::Report { body, pr } => report(&body, pr.as_deref())?,
        Command::Daemon { once } => daemon(&c.api, once).await?,
        Command::Webhook { url, secret } => webhook(&url, secret.as_deref())?,
        Command::Unhook => unhook()?,
        Command::List => request(&c.api, "/api/v1/starters", Method::GET, None).await?,
    }
    Ok(())
}
fn init() -> Result<(), Box<dyn std::error::Error>> {
    if !Path::new(".git").exists() {
        run_git(&["init", "-b", "main"])?;
    }
    if !Path::new(".gitignore").exists() {
        fs::write(".gitignore", ".starter/\n*.tmp\n")?;
    }
    println!("Initialized Starter repository on main.");
    Ok(())
}
async fn new_starter(api: &str, v: &str, id: &str) -> Result<(), Box<dyn std::error::Error>> {
    if !matches!(v, "public" | "private") {
        return Err("visibility must be public or private".into());
    }
    if local::is_repo(".") && local::load_binding(".").is_err() {
        let occupied = fs::read_dir(".")?.any(|entry| {
            entry
                .ok()
                .and_then(|e| e.file_name().into_string().ok())
                .is_some_and(|name| name != ".git" && name != ".gitignore")
        });
        if occupied {
            return Err(
                "refusing to create a Starter inside an existing non-Starter repository".into(),
            );
        }
    }
    if !local::is_repo(".") {
        init()?
    }
    if !silicon_starter_core::valid_id(id) {
        return Err("starter id may contain only letters, numbers, '.', '_' and '-'".into());
    }
    if !Path::new("silicon.yaml").exists() {
        fs::write("silicon.yaml", SEED_YAML)?
    }
    validate_silicon_yaml(&fs::read_to_string("silicon.yaml")?).map_err(|e| e.to_string())?;
    let value = authed_request_value(
        api,
        "/api/v1/starters",
        Method::POST,
        Some(json!({"id":id,"name":id,"description":"","visibility":v,"yaml":SEED_YAML})),
    )
    .await?;
    let binding = Binding {
        id: id.into(),
        api: api.into(),
        mode: Mode::Development,
        auto_update: false,
        pinned: None,
    };
    local::save_binding(".", &binding)?;
    local::register_checkout(".", &binding)?;
    if local::run_git(".", ["rev-parse", "HEAD"].as_ref()).is_err() {
        local::stage_and_commit(".", "Initialize starter")?;
    }
    println!("{value}");
    Ok(())
}
async fn login(api: &str, t: &str) -> Result<(), Box<dyn std::error::Error>> {
    if !t.starts_with("oac_") {
        return Err(
            "login requires an IAM oac_ short-lived token; credentials are never prompted".into(),
        );
    }
    let v = request_value(api, "/auth/cli", Method::POST, Some(json!({"slt":t}))).await?;
    let sid = v
        .get("session_id")
        .and_then(Value::as_str)
        .ok_or("IAM exchange response omitted session_id")?;
    let p = session_path();
    fs::create_dir_all(p.parent().unwrap())?;
    fs::write(p, sid)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(session_path(), fs::Permissions::from_mode(0o600))?;
    }
    println!("authenticated");
    Ok(())
}
async fn login_status(api: &str, j: bool) -> Result<(), Box<dyn std::error::Error>> {
    let sid = fs::read_to_string(session_path()).ok();
    if sid.is_none() {
        if j {
            println!("{{\"authenticated\":false}}")
        } else {
            println!("authenticated: false")
        }
        return Ok(());
    }
    let v = request_value_with_headers(api, "/auth/cli/status", Method::GET, None, sid.as_deref())
        .await?;
    if j {
        println!("{v}")
    } else {
        println!("authenticated: {}", v["authenticated"])
    }
    Ok(())
}
async fn push(api: &str, id: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
    let root = local::repo_root()?;
    let mut b = local::load_binding(&root)?;
    if b.mode != Mode::Development {
        return Err(
            "downloaded starters are read-only; use `starter pull` for an editable checkout".into(),
        );
    }
    if let Some(i) = id {
        b.id = i.into();
    }
    let yaml = local::validate_checkout(&root)?;
    let (path, commit) = local::bundle(&root)?;
    let body = json!({"commit":commit,"bundle_base64":B64.encode(fs::read(&path)?),"yaml":yaml,"message":"local main"});
    let result = authed_request_value(
        api,
        &format!("/api/v1/starters/{}/push", b.id),
        Method::POST,
        Some(body.clone()),
    )
    .await;
    let _ = fs::remove_file(path);
    match result {
        Ok(v) => println!("{v}"),
        Err(e) if e.to_string().contains("404") => {
            authed_request_value(api,"/api/v1/starters",Method::POST,Some(json!({"id":b.id,"name":b.id,"description":"","visibility":"public","yaml":yaml}))).await?;
            println!(
                "{}",
                authed_request_value(
                    api,
                    &format!("/api/v1/starters/{}/push", b.id),
                    Method::POST,
                    Some(body)
                )
                .await?
            );
        }
        Err(e) => return Err(e),
    }
    local::save_binding(&root, &b)?;
    local::register_checkout(&root, &b)?;
    Ok(())
}
async fn pull(
    api: &str,
    spec: Option<&str>,
    download: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let spec = spec.ok_or("pull requires a starter id on first use")?;
    let (id, rf) = spec
        .split_once('@')
        .map_or((spec, None), |(a, b)| (a, Some(b)));
    let q = rf.map_or(String::new(), |r| format!("?ref={}", encode(r)));
    let v = request_value(
        api,
        &format!("/api/v1/starters/{id}/archive{q}"),
        Method::GET,
        None,
    )
    .await?;
    let b = v
        .get("bundle_base64")
        .and_then(Value::as_str)
        .ok_or("archive response omitted bundle_base64")?;
    let response_commit = v
        .get("commit")
        .and_then(Value::as_str)
        .ok_or("archive response omitted commit")?;
    let temp = std::env::temp_dir().join(format!(
        "starter-{}-{}.bundle",
        std::process::id(),
        local::unique()
    ));
    fs::write(&temp, B64.decode(b)?)?;
    let commit = if local::is_hex_commit(response_commit) {
        response_commit.to_owned()
    } else {
        local::bundle_head(&temp)?
    };
    let current_binding = local::load_binding(".").ok();
    let target = if current_binding.as_ref().is_some_and(|b| b.id == id) {
        PathBuf::from(".")
    } else {
        PathBuf::from(id.rsplit('.').next().unwrap_or(id))
    };
    if local::is_repo(&target) {
        let target = target
            .canonicalize()
            .map_err(|e| format!("cannot resolve Starter checkout: {e}"))?;
        let mut binding = local::load_binding(&target)
            .map_err(|_| "target is an existing non-Starter git repository")?;
        local::ensure_main(&target)?;
        if !run_git_dir(&target, ["status", "--porcelain"].as_ref())?.is_empty() {
            local::stage_and_commit(&target, "Local changes before Starter update")?;
        }
        let merged = merge_transaction(&target, &temp);
        let _ = fs::remove_file(&temp);
        merged?;
        binding.mode = if download {
            Mode::Download
        } else {
            Mode::Development
        };
        binding.auto_update = download;
        binding.pinned = rf.map(str::to_owned);
        local::save_binding(&target, &binding)?;
        local::register_checkout(&target, &binding)?;
        println!("updated {id} at {commit}");
        return Ok(());
    }
    local::clone_bundle(&temp, &target, &commit)?;
    let _ = fs::remove_file(temp);
    let binding = Binding {
        id: id.into(),
        api: api.into(),
        mode: if download {
            Mode::Download
        } else {
            Mode::Development
        },
        auto_update: download,
        pinned: rf.map(str::to_owned),
    };
    local::save_binding(&target, &binding)?;
    local::register_checkout(&target, &binding)?;
    println!("installed {id} at {commit}");
    Ok(())
}
async fn publish(
    api: &str,
    sel: &str,
    ver: &str,
    notes: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    let b = local::load_binding(".")?;
    if b.mode != Mode::Development {
        return Err("only pulled developer starters may publish".into());
    }
    let c = if sel == "latest" {
        local::head(".")?
    } else {
        local::run_git(".", ["rev-parse", &format!("{sel}^{{commit}}")].as_ref())?
    };
    println!(
        "{}",
        authed_request_value(
            api,
            &format!("/api/v1/starters/{}/publish", b.id),
            Method::POST,
            Some(json!({"selector":sel,"version":ver,"commit":c,"notes":notes.unwrap_or("")}))
        )
        .await?
    );
    Ok(())
}
async fn update(api: &str, mode: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut b = local::load_binding(".")?;
    match mode {
        "on" => {
            if b.mode == Mode::Development {
                return Err("developer pull checkouts never auto-update".into());
            }
            b.auto_update = true
        }
        "off" => b.auto_update = false,
        "now" => {
            if b.mode == Mode::Development {
                return Err("developer pull checkouts never auto-update".into());
            }
            if let Some(pin) = &b.pinned {
                return Err(format!("starter is pinned to {pin}; turn the pin off by downloading the unqualified starter" ).into());
            }
            return pull(api, Some(&b.id), true).await;
        }
        _ => return Err("update expects on, off, or now".into()),
    }
    local::save_binding(".", &b)?;
    println!("auto-update {}", if b.auto_update { "on" } else { "off" });
    Ok(())
}
async fn daemon(api: &str, once: bool) -> Result<(), Box<dyn std::error::Error>> {
    loop {
        let file = local::registry_path();
        let entries: Vec<local::RegistryEntry> = fs::read(&file)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        let mut active = 0;
        for entry in entries {
            if entry.binding.api != api
                || entry.binding.mode != Mode::Download
                || !entry.binding.auto_update
                || entry.binding.pinned.is_some()
            {
                continue;
            }
            active += 1;
            let result = std::env::current_exe().and_then(|exe| {
                Process::new(exe)
                    .current_dir(&entry.path)
                    .args(["--api", api, "update", "now"])
                    .status()
            });
            match result {
                Ok(status) if status.success() => {}
                Ok(status) => eprintln!(
                    "auto-update failed for {}: exit {}",
                    entry.binding.id, status
                ),
                Err(error) => eprintln!(
                    "auto-update failed for {}: could not run starter: {}",
                    entry.binding.id, error
                ),
            }
        }
        if active == 0 {
            println!("daemon: no active auto-updating downloaded starters");
        }
        if once {
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
    }
}

/// Merge an archive in a disposable clone, then fast-forward the user's checkout.
/// The real checkout is touched only after the merge (and any Omni repair) succeeds.
fn merge_transaction(target: &Path, bundle: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let stage = std::env::temp_dir().join(format!(
        "starter-update-{}-{}",
        std::process::id(),
        local::unique()
    ));
    let result = (|| -> Result<String, Box<dyn std::error::Error>> {
        let parent = stage.parent().unwrap_or_else(|| Path::new("."));
        local::run_git(
            parent,
            [
                "clone",
                "--no-hardlinks",
                target.to_str().ok_or("repository path is not UTF-8")?,
                stage.to_str().ok_or("temporary path is not UTF-8")?,
            ]
            .as_ref(),
        )?;
        local::ensure_main(&stage)?;
        local::run_git(
            &stage,
            [
                "fetch",
                bundle.to_str().ok_or("bundle path is not UTF-8")?,
                "refs/heads/main:refs/remotes/starter/main",
            ]
            .as_ref(),
        )?;
        let remote = "refs/remotes/starter/main";
        let merge = local::run_git(&stage, ["merge", "--ff-only", remote].as_ref());
        if merge.is_err()
            && let Err(error) = local::run_git(&stage, ["merge", "--no-edit", remote].as_ref())
        {
            resolve_with_omni(&stage, &error)?;
        }
        if !local::run_git(&stage, ["ls-files", "-u"].as_ref())?.is_empty() {
            return Err("update left unresolved merge entries".into());
        }
        if has_conflict_markers(&stage) {
            return Err("update left merge conflict markers".into());
        }
        let commit = local::head(&stage)?;
        local::run_git(
            target,
            [
                "fetch",
                stage.to_str().ok_or("temporary path is not UTF-8")?,
                "refs/heads/main:refs/remotes/starter/transaction",
            ]
            .as_ref(),
        )?;
        local::run_git(
            target,
            ["reset", "--hard", "refs/remotes/starter/transaction"].as_ref(),
        )?;
        Ok(commit)
    })();
    let _ = fs::remove_dir_all(&stage);
    result
}

fn resolve_with_omni(stage: &Path, merge_error: &str) -> Result<(), Box<dyn std::error::Error>> {
    let sibling = std::env::current_exe()
        .ok()
        .and_then(|p| {
            p.parent()?
                .parent()?
                .parent()?
                .parent()
                .map(Path::to_path_buf)
        })
        .map(|p| p.join("silicon-omni/target/release/silicon-omni"));
    let binary = [
        std::env::var_os("SILICON_OMNI").map(PathBuf::from),
        Some(PathBuf::from("silicon-omni")),
        Some(PathBuf::from("so")),
        sibling,
        Some(PathBuf::from("../silicon-omni/target/release/silicon-omni")),
        Some(PathBuf::from("../silicon-omni/target/debug/silicon-omni")),
    ]
    .into_iter()
    .flatten()
    .find(|candidate| {
        candidate.components().count() > 1 || Process::new(candidate).arg("--help").output().is_ok()
    })
    .ok_or("merge conflict needs silicon-omni, but no silicon-omni executable was found")?;
    let binary = if binary.components().count() > 1 {
        binary
            .canonicalize()
            .map_err(|_| "silicon-omni executable path does not exist")?
    } else {
        binary
    };
    let listed = Process::new(&binary).arg("providers").output()?;
    if !listed.status.success() {
        return Err("silicon-omni could not list an available provider".into());
    }
    let providers: Vec<String> = String::from_utf8_lossy(&listed.stdout)
        .lines()
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(ToOwned::to_owned)
        .collect();
    if providers.is_empty() {
        return Err(
            "merge conflict needs an installed, authenticated silicon-omni provider".into(),
        );
    }
    let session = format!("starter-merge-{}-{}", std::process::id(), local::unique());
    let prompt = format!(
        "Resolve the git merge conflict in this checkout. Inspect every conflict, preserve the intended changes from both sides, remove all conflict markers, then stage and commit the complete result with message `Resolve Starter update conflict`. Do not merely explain; edit the files. Git reported: {merge_error}"
    );
    let mut last_error = String::new();
    for provider in providers {
        let out = Process::new(&binary)
            .current_dir(stage)
            .args([
                "send",
                "--key",
                "general",
                "--providers",
                &provider,
                &session,
                &prompt,
            ])
            .output();
        match out {
            Ok(out) if out.status.success() => {
                wait_omni_idle(&binary, &session);
                if local::run_git(stage, ["ls-files", "-u"].as_ref())?.is_empty() {
                    if has_conflict_markers(stage) {
                        last_error = "provider left merge conflict markers".into();
                        continue;
                    }
                    if !local::run_git(stage, ["status", "--porcelain"].as_ref())?.is_empty() {
                        local::stage_and_commit(stage, "Resolve Starter update conflict")?;
                    }
                    if local::run_git(stage, ["status", "--porcelain"].as_ref())?.is_empty() {
                        return Ok(());
                    }
                }
            }
            Ok(out) => last_error = String::from_utf8_lossy(&out.stderr).trim().to_string(),
            Err(error) => last_error = error.to_string(),
        }
    }
    let _ = local::run_git(stage, ["merge", "--abort"].as_ref());
    Err(format!("silicon-omni could not resolve the merge conflict: {last_error}").into())
}

fn has_conflict_markers(stage: &Path) -> bool {
    local::run_git(
        stage,
        ["grep", "-nE", "^(<<<<<<<|=======|>>>>>>>)", "--", "."].as_ref(),
    )
    .is_ok()
}

fn wait_omni_idle(binary: &Path, session: &str) {
    for _ in 0..60 {
        let Ok(out) = Process::new(binary).args(["status", session]).output() else {
            return;
        };
        if !out.status.success() {
            return;
        }
        let Ok(value) = serde_json::from_slice::<Value>(&out.stdout) else {
            return;
        };
        let snapshot = value.get("snapshot").unwrap_or(&value);
        if snapshot.get("status").and_then(Value::as_str) == Some("waiting")
            && snapshot.get("in_turn").and_then(Value::as_bool) != Some(true)
        {
            return;
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}

fn revert(t: &str) -> Result<(), Box<dyn std::error::Error>> {
    let r = local::repo_root()?;
    if !local::run_git(&r, ["status", "--porcelain"].as_ref())?.is_empty() {
        local::stage_and_commit(&r, "Local changes before Starter revert")?;
    }
    let c = local::run_git(&r, ["rev-parse", &format!("{t}^{{commit}}")].as_ref())?;
    let stage = std::env::temp_dir().join(format!(
        "starter-revert-{}-{}",
        std::process::id(),
        local::unique()
    ));
    let parent = stage.parent().unwrap_or_else(|| Path::new("."));
    local::run_git(
        parent,
        [
            "clone",
            "--no-hardlinks",
            r.to_str().ok_or("repository path is not UTF-8")?,
            stage.to_str().ok_or("temporary path is not UTF-8")?,
        ]
        .as_ref(),
    )?;
    let result = (|| -> Result<String, Box<dyn std::error::Error>> {
        local::ensure_main(&stage)?;
        // Reset only the disposable index/worktree to the requested tree. The
        // current branch and all history remain in place until the new commit.
        local::run_git(&stage, ["read-tree", "--reset", "-u", &c].as_ref())?;
        local::run_git(&stage, ["clean", "-fd"].as_ref())?;
        local::run_git(&stage, ["add", "-A"].as_ref())?;
        local::run_git(
            &stage,
            ["commit", "--allow-empty", "-m", &format!("Revert to {t}")].as_ref(),
        )?;
        let new_head = local::head(&stage)?;
        local::run_git(
            &r,
            [
                "fetch",
                stage.to_str().ok_or("temporary path is not UTF-8")?,
                "refs/heads/main:refs/remotes/starter/revert",
            ]
            .as_ref(),
        )?;
        local::run_git(
            &r,
            ["reset", "--hard", "refs/remotes/starter/revert"].as_ref(),
        )?;
        Ok(new_head)
    })();
    let _ = fs::remove_dir_all(&stage);
    if let Ok(ref new_head) = result {
        println!("reverted to {c}; new commit {new_head}");
    }
    result.map(|_| ())
}
fn report(body: &str, pr: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
    let title = if let Some(p) = pr {
        format!("Starter CLI bug report (PR {p})")
    } else {
        "Starter CLI bug report".into()
    };
    let out = Process::new("gh")
        .args(["issue", "create", "--title", &title, "--body", body])
        .output()?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr)
            .trim()
            .to_string()
            .into());
    }
    print!("{}", String::from_utf8_lossy(&out.stdout));
    Ok(())
}
fn webhook(url: &str, secret: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
    if !(url.starts_with("https://") || url.starts_with("http://127.0.0.1")) {
        return Err("webhook URL must use https (or localhost for development)".into());
    }
    let path = webhook_path();
    fs::create_dir_all(path.parent().unwrap())?;
    fs::write(
        path,
        serde_json::to_vec(&json!({"url":url,"secret":secret}))?,
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(webhook_path(), fs::Permissions::from_mode(0o600))?;
    }
    println!("webhook configured for the daemon");
    Ok(())
}
fn unhook() -> Result<(), Box<dyn std::error::Error>> {
    let path = webhook_path();
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    println!("webhook removed");
    Ok(())
}
fn webhook_path() -> PathBuf {
    std::env::var_os("SILICON_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".starter/webhook.json")
}
fn session_path() -> PathBuf {
    std::env::var_os("SILICON_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".starter/session")
}
async fn authed_request_value(
    api: &str,
    p: &str,
    m: Method,
    b: Option<Value>,
) -> Result<Value, Box<dyn std::error::Error>> {
    let s = fs::read_to_string(session_path())
        .map_err(|_| "not authenticated; run `starter login <SLT>`")?;
    request_value_with_headers(api, p, m, b, Some(&s)).await
}
async fn authed_request(
    api: &str,
    p: &str,
    m: Method,
    b: Option<Value>,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("{}", authed_request_value(api, p, m, b).await?);
    Ok(())
}
async fn request_value(
    api: &str,
    p: &str,
    m: Method,
    b: Option<Value>,
) -> Result<Value, Box<dyn std::error::Error>> {
    request_value_with_headers(api, p, m, b, None).await
}
async fn request_value_with_headers(
    api: &str,
    p: &str,
    m: Method,
    b: Option<Value>,
    s: Option<&str>,
) -> Result<Value, Box<dyn std::error::Error>> {
    let c = reqwest::Client::new();
    let mut r = c.request(m, format!("{api}{p}"));
    if let Some(s) = s {
        r = r.header("x-starter-session", s.trim())
    }
    if let Some(v) = b {
        r = r.json(&v)
    }
    let x = r.send().await?;
    let status = x.status();
    let text = x.text().await?;
    let v = serde_json::from_str(&text).unwrap_or_else(|_| json!({"body":text}));
    if !status.is_success() {
        return Err(format!("HTTP {status}: {v}").into());
    }
    Ok(v)
}
async fn request(
    api: &str,
    p: &str,
    m: Method,
    b: Option<Value>,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("{}", request_value(api, p, m, b).await?);
    Ok(())
}
fn run_git(a: &[&str]) -> Result<String, Box<dyn std::error::Error>> {
    local::run_git(".", a).map_err(Into::into)
}
fn run_git_dir(dir: &Path, a: &[&str]) -> Result<String, Box<dyn std::error::Error>> {
    local::run_git(dir, a).map_err(Into::into)
}
fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}
