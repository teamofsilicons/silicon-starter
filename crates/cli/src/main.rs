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
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::Command as Process,
};

mod blocks;
mod publication;
mod session;
mod templates;

#[derive(Parser)]
#[command(
    name = "starter",
    version,
    about = "Version controlled silicon starter registry. Use --help to explore the command tree."
)]
struct Cli {
    #[arg(
        long,
        global = true,
        env = "STARTER_API_URL",
        default_value = "https://backend.starter.teamofsilicons.com"
    )]
    api: String,
    /// Independent saved Carbon or Silicon account profile.
    #[arg(
        long,
        global = true,
        env = "STARTER_PROFILE",
        default_value = "default"
    )]
    profile: String,
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    Accounts {
        #[arg(long)]
        json: bool,
    },
    Login {
        token: Option<String>,
        /// A short-lived Silicon Accounts token requested for starter.
        #[arg(long, conflicts_with_all = ["token", "slt_stdin"])]
        slt: Option<String>,
        /// Read the SLT from stdin without putting it in shell history or process arguments.
        #[arg(long, conflicts_with = "token")]
        slt_stdin: bool,
        /// Recover the exact interrupted login, without entering another SLT.
        #[arg(long, conflicts_with_all=["token","slt","slt_stdin","cancel"])]
        recover: bool,
        /// Discard the private local login retry receipt.
        #[arg(long, conflicts_with_all=["token","slt","slt_stdin","recover"])]
        cancel: bool,
        #[command(subcommand)]
        command: Option<LoginCommand>,
    },
    /// Revoke the saved session and sign out of this profile.
    Logout,
    /// Inspect or explicitly bind the current checkout to a saved account.
    Context {
        #[command(subcommand)]
        command: ContextCommand,
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
        /// Destination for a new checkout; must not already be occupied.
        #[arg(long)]
        dir: Option<PathBuf>,
        #[command(flatten)]
        seed: templates::SeedOptions,
    },
    Download {
        id: String,
        #[arg(long)]
        dir: Option<PathBuf>,
        #[command(flatten)]
        seed: templates::SeedOptions,
    },
    /// Rebuild .starterbase with saved answers, optionally changing or resetting them.
    Seed {
        #[command(flatten)]
        options: templates::SeedOptions,
        /// Validate the recipe and templates without executing commands or changing files.
        #[arg(long)]
        check: bool,
    },
    Publish {
        /// latest/commit for starters, history, or a gene:/isi:/function: ID.
        selector: Option<String>,
        /// Starter release Y.X, block Markdown/ZIP file, or block ID after history.
        version: Option<String>,
        #[arg(long)]
        notes: Option<String>,
        #[command(flatten)]
        block: blocks::PublishOptions,
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
        #[arg(long, hide = true)]
        background: bool,
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
enum ContextCommand {
    Show,
    Bind,
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
    let mut c = Cli::parse();
    let selection = session::Selection::new(&c.api, &c.profile)?;
    c.api = selection.api.clone();
    let ignore_saved = matches!(
        &c.command,
        Command::Login { command: None, .. }
            | Command::Accounts { .. }
            | Command::Init
            | Command::Commit { .. }
            | Command::Seed { .. }
            | Command::Revert { .. }
            | Command::Report { .. }
            | Command::Daemon { .. }
            | Command::Webhook { .. }
            | Command::Unhook
            | Command::Context {
                command: ContextCommand::Show
            }
    );
    session::configure(selection, ignore_saved)?;
    match c.command {
        Command::Accounts { json: true } => println!(
            "{}",
            serde_json::to_string_pretty(
                &json!({"app_id":"starter","base_url":c.api,"docs":"https://starter.teamofsilicons.com/docs","source":"https://github.com/teamofsilicons/silicon-starter","package":"https://crates.io/crates/silicon-starter-core","accounts_url":"https://accounts.teamofsilicons.com","apps_url":"https://apps.teamofsilicons.com/apps/starter"})
            )?
        ),
        Command::Accounts { json: false } => println!(
            "starter\nAPI: {}\nDocs: https://starter.teamofsilicons.com/docs",
            c.api
        ),
        Command::Login {
            token,
            slt,
            slt_stdin,
            command,
            recover,
            cancel,
        } => match command {
            Some(LoginCommand::Status { json })
                if token.is_none() && slt.is_none() && !slt_stdin && !recover && !cancel =>
            {
                session::status(json).await?
            }
            None if cancel => session::cancel_login().await?,
            None if recover => session::recover_login().await?,
            None => {
                if slt_stdin {
                    let mut token = String::new();
                    io::stdin().take(8193).read_to_string(&mut token)?;
                    if token.len() > 8192 {
                        return Err("SLT must be at most 8192 bytes".into());
                    }
                    session::login(Some(token.trim())).await?;
                } else if let Some(token) = token.or(slt) {
                    session::login(Some(&token)).await?;
                } else {
                    session::device_login(false).await?;
                }
            }
            _ => {
                return Err(
                    "usage: starter login --slt <SLT> | starter login status --json".into(),
                );
            }
        },
        Command::Logout => session::logout().await?,
        Command::Context { command } => checkout_context(command)?,
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
        Command::Pull { id, dir, seed } => {
            pull(&c.api, id.as_deref(), false, dir.as_deref(), &seed).await?
        }
        Command::Download { id, dir, seed } => {
            if blocks::is_block(&id) {
                blocks::download(&c.api, &id, dir.as_deref()).await?
            } else {
                pull(&c.api, Some(&id), true, dir.as_deref(), &seed).await?
            }
        }
        Command::Seed { options, check } => templates::seed(&options, check)?,
        Command::Publish {
            selector,
            version,
            notes,
            block,
        } => {
            if selector.as_deref() == Some("history") {
                if let Some(id) = version {
                    blocks::history(&c.api, &id).await?;
                } else {
                    let b = local::load_binding(".")?;
                    request(
                        &c.api,
                        &format!("/api/v1/starters/{}/versions", b.id),
                        Method::GET,
                        None,
                    )
                    .await?
                }
            } else if selector.as_deref().is_some_and(blocks::is_block) {
                blocks::publish(
                    &c.api,
                    selector.as_deref().unwrap(),
                    version.as_deref(),
                    &block,
                )
                .await?;
            } else {
                publication::run(
                    &selector.ok_or("publish requires latest, a commit prefix, or history")?,
                    version.as_deref(),
                    notes.as_deref(),
                )
                .await?;
            }
        }
        Command::Update { mode } => update(&c.api, &mode).await?,
        Command::Revert { target } => revert(&target)?,
        Command::History { kind } => match kind.as_deref().unwrap_or("commit") {
            "commit" => println!("{}", local::commit_history(local::repo_root()?, 100)?),
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
            id if blocks::is_block(id) => blocks::history(&c.api, id).await?,
            x => {
                return Err(format!(
                    "unknown history kind `{x}`; expected commit, publish, or a block ID"
                )
                .into());
            }
        },
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
            if blocks::is_block(&id) {
                silicon_starter_core::blocks::parse_id(&id)?;
                request(&c.api, &format!("/api/v1/blocks/{id}"), Method::GET, None).await?
            } else {
                request(&c.api, &format!("/api/v1/starters/{id}"), Method::GET, None).await?
            }
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
        Command::Daemon { once, background } => daemon(&c.api, once, background).await?,
        Command::Webhook { url, secret } => webhook(&url, secret.as_deref())?,
        Command::Unhook => unhook()?,
        Command::List => request(&c.api, "/api/v1/starters", Method::GET, None).await?,
    }
    Ok(())
}
fn checkout_context(command: ContextCommand) -> session::Result<()> {
    let root = local::repo_root()?;
    let mut binding = local::load_binding(&root)?;
    if matches!(command, ContextCommand::Bind) {
        if session::normalize_api(&binding.api)? != session::current()?.selection.api {
            return Err("use the checkout's original --api before binding its account".into());
        }
        session::current()?.check_current()?;
        binding.auth_context = Some(session::current()?.binding());
        local::save_binding(&root, &binding)?;
        let registered = local::registered_checkouts()?
            .iter()
            .any(|entry| entry.path == root);
        local::register_checkout(&root, &binding, registered)?;
    }
    println!("{}", serde_json::to_value(&binding.auth_context)?);
    Ok(())
}
fn init() -> Result<(), Box<dyn std::error::Error>> {
    if !Path::new(".git").exists() {
        run_git(&["init", "-b", "main"])?;
    }
    if !Path::new(".gitignore").exists() {
        fs::write(".gitignore", ".starter/\n.starterbase/.state/\n*.tmp\n")?;
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
        return Err(
            "starter id must be handle.name; the name uses lowercase letters, numbers and hyphens"
                .into(),
        );
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
        auth_context: Some(session::current()?.binding()),
        id: id.into(),
        api: api.into(),
        mode: Mode::Development,
        pinned: None,
    };
    local::save_binding(".", &binding)?;
    local::register_checkout(".", &binding, false)?;
    if local::run_git(".", ["rev-parse", "HEAD"].as_ref()).is_err() {
        local::stage_and_commit(".", "Initialize starter")?;
    }
    println!("{value}");
    Ok(())
}
async fn push(api: &str, id: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
    let root = local::repo_root()?;
    let mut b = match local::load_binding(&root) {
        Ok(binding) => binding,
        Err(_) if id.is_some() => Binding {
            auth_context: Some(session::current()?.binding()),
            id: id.unwrap().into(),
            api: api.into(),
            mode: Mode::Development,
            pinned: None,
        },
        Err(error) => return Err(error.into()),
    };
    if b.mode != Mode::Development {
        return Err(
            "downloaded starters are read-only; use `starter pull` for an editable checkout".into(),
        );
    }
    if let Some(i) = id {
        b.id = i.into();
    }
    if !silicon_starter_core::valid_id(&b.id) {
        return Err("invalid starter id".into());
    }
    let context = session::current()?.for_checkout(&b)?;
    if context.saved.is_none() {
        return Err("pushing requires a saved account; explicitly use starter context bind for this checkout".into());
    }
    local::ensure_publishable_history(&root)?;
    templates::prepare_push(&root, &b.id)?;
    let yaml = local::validate_checkout(&root)?;
    let (path, commit) = local::bundle(&root)?;
    let body = json!({"commit":commit,"bundle_base64":B64.encode(fs::read(&path)?),"yaml":yaml,"message":"local main"});
    let result = context
        .request(
            &format!("/api/v1/starters/{}/push", b.id),
            Method::POST,
            Some(body.clone()),
            None,
            true,
        )
        .await;
    let _ = fs::remove_file(path);
    match result {
        Ok(v) => println!("{v}"),
        Err(e) if e.to_string().contains("404") => {
            context.request("/api/v1/starters",Method::POST,Some(json!({"id":b.id,"name":b.id,"description":"","visibility":"public","yaml":yaml})),None,true).await?;
            println!(
                "{}",
                context
                    .request(
                        &format!("/api/v1/starters/{}/push", b.id),
                        Method::POST,
                        Some(body),
                        None,
                        true
                    )
                    .await?
            );
        }
        Err(e) => return Err(e),
    }
    local::save_binding(&root, &b)?;
    local::register_checkout(&root, &b, false)?;
    Ok(())
}
async fn pull(
    api: &str,
    spec: Option<&str>,
    download: bool,
    destination: Option<&Path>,
    options: &templates::SeedOptions,
) -> Result<(), Box<dyn std::error::Error>> {
    let root = local::repo_root().unwrap_or_else(|_| PathBuf::from("."));
    let current_binding = local::load_binding(&root).ok();
    let bound_spec = current_binding.as_ref().map(|binding| {
        binding
            .pinned
            .as_ref()
            .map_or_else(|| binding.id.clone(), |pin| format!("{}@{pin}", binding.id))
    });
    let spec = spec
        .or(bound_spec.as_deref())
        .ok_or("pull requires a starter id on first use")?;
    let (id, reference) = spec
        .split_once('@')
        .map_or((spec, None), |(a, b)| (a, Some(b)));
    if !silicon_starter_core::valid_id(id) || reference == Some("") {
        return Err("invalid starter id or empty version reference".into());
    }
    let existing = destination.is_none() && current_binding.as_ref().is_some_and(|b| b.id == id);
    let target = if existing {
        root
    } else if let Some(path) = destination {
        path.to_path_buf()
    } else {
        let default = id.rsplit('.').next().unwrap_or(id);
        if options.interactive() {
            print!("Folder name [{default}]: ");
            io::stdout().flush()?;
            let mut name = String::new();
            io::stdin().read_line(&mut name)?;
            PathBuf::from(if name.trim().is_empty() {
                default
            } else {
                name.trim()
            })
        } else {
            PathBuf::from(default)
        }
    };
    if !existing && target.exists() && fs::read_dir(&target)?.next().is_some() {
        return Err(format!(
            "destination {} is occupied; choose another folder with --dir",
            target.display()
        )
        .into());
    }
    let query = if download {
        reference.map_or(String::new(), |r| format!("?ref={}", encode(r)))
    } else {
        format!(
            "?mode=pull{}",
            reference.map_or(String::new(), |r| format!("&ref={}", encode(r)))
        )
    };
    let context = if existing {
        session::current()?.for_checkout(
            current_binding
                .as_ref()
                .ok_or("checkout binding disappeared")?,
        )?
    } else {
        session::current()?.clone()
    };
    let archive = context
        .request(
            &format!("/api/v1/starters/{id}/archive{query}"),
            Method::GET,
            None,
            None,
            !download,
        )
        .await?;
    let temp = templates::Temporary::new("archive.bundle");
    fs::write(
        &temp.0,
        B64.decode(
            archive
                .get("bundle_base64")
                .and_then(Value::as_str)
                .ok_or("archive response omitted bundle_base64")?,
        )?,
    )?;
    let response_commit = archive
        .get("commit")
        .and_then(Value::as_str)
        .ok_or("archive response omitted commit")?;
    if !local::is_hex_commit(response_commit) {
        return Err("archive response must contain a full Git commit ID".into());
    }
    let commit = response_commit.to_owned();
    if existing {
        let target = target.canonicalize()?;
        let mut binding = local::load_binding(&target)?;
        local::ensure_main(&target)?;
        if download && binding.mode == Mode::Development {
            return Err(
                "this is a developer checkout; use --dir to create a separate, unpushable download"
                    .into(),
            );
        }
        let previous = if binding.mode == Mode::Download && templates::exists(&target) {
            match silicon_starter_core::seed::load_state(&target) {
                Ok(state) => state.map(|s| s.revision),
                Err(error) => {
                    pause_updates(&target, &binding)?;
                    return Err(format!(
                        "update was not applied; automatic updates are off: {error}"
                    )
                    .into());
                }
            }
        } else {
            Some(local::head(&target)?)
        };
        if reference.is_some()
            && !templates::exists(&target)
            && previous.as_deref() != Some(&commit)
            && local::run_git(&target, &["merge-base", "--is-ancestor", &commit, "HEAD"]).is_ok()
        {
            return Err("this version is older than the existing checkout; use --dir to download it into a separate folder".into());
        }
        if previous.as_deref() != Some(&commit) {
            if !run_git_dir(&target, &["status", "--porcelain"])?.is_empty() {
                if binding.mode == Mode::Development {
                    return Err(
                        "commit your source changes before pulling into a developer checkout"
                            .into(),
                    );
                }
                templates::commit(&target, "Local changes before Starter update")?;
            }
            let result = if binding.mode == Mode::Download && templates::exists(&target) {
                templates::update(&target, &temp.0, &commit, id)
            } else {
                merge_transaction(&target, &temp.0, &commit).map(|_| ())
            };
            if let Err(error) = result {
                pause_updates(&target, &binding)?;
                return Err(
                    format!("update was not applied; automatic updates are off: {error}").into(),
                );
            }
        }
        // Checkout modes are permanent: downloaded history may contain private
        // instance configuration and must never become pushable.
        binding.pinned = reference.map(str::to_owned);
        local::save_binding(&target, &binding)?;
        if binding.pinned.is_some() || binding.mode == Mode::Development {
            local::register_checkout(&target, &binding, false)?;
        }
        println!("updated {id} at {commit}");
        return Ok(());
    }
    local::clone_bundle(&temp.0, &target, &commit)?;
    let installed = (|| -> Result<(), Box<dyn std::error::Error>> {
        if !templates::exists(&target) {
            return Ok(());
        }
        let state = templates::install(&target, options, id, &commit)?;
        templates::trim_install(&target, &state)?;
        if download {
            templates::commit(&target, "Seed Starter instance")?;
        }
        Ok(())
    })();
    if let Err(error) = installed {
        let _ = fs::remove_dir_all(&target);
        return Err(error);
    }
    let auto_update = download && reference.is_none();
    let binding = Binding {
        auth_context: Some(session::current()?.binding()),
        id: id.into(),
        api: api.into(),
        mode: if download {
            Mode::Download
        } else {
            Mode::Development
        },
        pinned: reference.map(str::to_owned),
    };
    local::save_binding(&target, &binding)?;
    local::register_checkout(&target, &binding, auto_update)?;
    if auto_update {
        start_daemon(api)?;
    }
    println!(
        "installed {id} at {commit} in {} (automatic updates {})",
        target.display(),
        if auto_update { "on" } else { "off" }
    );
    Ok(())
}

fn pause_updates(target: &Path, binding: &Binding) -> Result<(), Box<dyn std::error::Error>> {
    local::register_checkout(target, binding, false)?;
    Ok(())
}

async fn update(api: &str, mode: &str) -> Result<(), Box<dyn std::error::Error>> {
    let root = local::repo_root()?;
    let mut binding = local::load_binding(&root)?;
    match mode {
        "on" => {
            if binding.mode == Mode::Development {
                return Err("developer pull checkouts never auto-update".into());
            }
            binding.pinned = None;
        }
        "off" => {}
        "now" => {
            if binding.mode == Mode::Development {
                return Err("developer pull checkouts never auto-update".into());
            }
            if let Some(pin) = &binding.pinned {
                return Err(format!(
                    "starter is pinned to {pin}; use `starter update on` to resume latest releases"
                )
                .into());
            }
            return pull(
                api,
                Some(&binding.id),
                true,
                None,
                &templates::SeedOptions::unattended(),
            )
            .await;
        }
        _ => return Err("update expects on, off, or now".into()),
    }
    local::save_binding(&root, &binding)?;
    local::register_checkout(&root, &binding, mode == "on")?;
    if mode == "on" {
        start_daemon(api)?;
    }
    println!("auto-update {mode}");
    Ok(())
}

fn start_daemon(api: &str) -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var_os("STARTER_NO_DAEMON").is_some() {
        return Ok(());
    }
    let directory = local::data_dir();
    fs::create_dir_all(&directory)?;
    let log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(directory.join("daemon.log"))?;
    let mut command = Process::new(std::env::current_exe()?);
    command
        .args(["--api", api, "daemon", "--background"])
        .current_dir(&directory)
        .stdin(std::process::Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x00000008 | 0x00000200);
    }
    command.spawn()?;
    Ok(())
}

async fn daemon(api: &str, once: bool, background: bool) -> Result<(), Box<dyn std::error::Error>> {
    let Some(mut lock) = session::daemon_lock()? else {
        return Ok(());
    };
    lock.set_len(0)?;
    writeln!(lock, "{}", std::process::id())?;
    if background && !once {
        tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
    }
    loop {
        let entries = local::registered_checkouts()?;
        let mut active = 0;
        for entry in entries {
            let Ok(binding) = local::load_binding(&entry.path) else {
                continue;
            };
            if once && binding.api != api {
                continue;
            }
            if binding.mode != Mode::Download || binding.pinned.is_some() {
                continue;
            }
            active += 1;
            let profile = binding
                .auth_context
                .as_ref()
                .map_or("default", |context| context.profile.as_str());
            let result = std::env::current_exe().and_then(|exe| {
                Process::new(exe)
                    .current_dir(&entry.path)
                    .args(["--api", &binding.api, "--profile", profile, "update", "now"])
                    .stdin(std::process::Stdio::null())
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
fn merge_transaction(
    target: &Path,
    bundle: &Path,
    commit: &str,
) -> Result<String, Box<dyn std::error::Error>> {
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
        let merge = local::run_git(&stage, ["merge", "--ff-only", commit].as_ref());
        if merge.is_err()
            && let Err(error) = local::run_git(&stage, ["merge", "--no-edit", commit].as_ref())
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
    // Equals-only lines are also ordinary text dividers; check the other markers.
    local::run_git(
        stage,
        [
            "grep",
            "-IqE",
            r"^(<{7,}|>{7,}|\|{7,})([[:space:]]|$)",
            "--",
            ".",
        ]
        .as_ref(),
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
    local::data_dir().join("webhook.json")
}
async fn authed_request_value(
    api: &str,
    p: &str,
    m: Method,
    b: Option<Value>,
) -> Result<Value, Box<dyn std::error::Error>> {
    if session::normalize_api(api)? != session::current()?.selection.api {
        return Err("request API changed after context selection".into());
    }
    session::current()?.request(p, m, b, None, true).await
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
    if session::normalize_api(api)? != session::current()?.selection.api {
        return Err("request API changed after context selection".into());
    }
    session::current()?.request(p, m, b, None, false).await
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
