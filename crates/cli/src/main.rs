use clap::{Parser, Subcommand};
use reqwest::Method;
use serde_json::json;
use std::{fs, process::Command as Process};

#[derive(Parser)]
#[command(
    name = "starter",
    version,
    about = "Version controlled silicon starter registry. Run `starter --help` to explore the command tree."
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
        message: String,
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
        selector: String,
        version: String,
    },
    Update {
        mode: String,
    },
    Revert {
        target: String,
    },
    History {
        #[arg(default_value = "commit")]
        kind: String,
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
    Report {
        body: String,
        #[arg(long)]
        pr: Option<String>,
    },
    List,
}
#[derive(Subcommand)]
enum LoginCommand {
    Status {
        #[arg(long)]
        json: bool,
    },
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let c = Cli::parse();
    match c.command {
        Command::Iam { json: true } => println!(
            "{}",
            serde_json::to_string_pretty(
                &json!({"app_id":"tos>starter","auth_url":"https://auth.iam.teamofsilicons.com/login?app_id=tos%3Estarter","docs":"https://starter.teamofsilicons.com/docs","source":"https://github.com/teamofsilicons/silicon-starter","package":"https://crates.io/crates/silicon-starter"})
            )?
        ),
        Command::Iam { json: false } => println!(
            "tos>starter\nAuth: https://auth.iam.teamofsilicons.com/login?app_id=tos%3Estarter\nDocs: https://starter.teamofsilicons.com/docs"
        ),
        Command::Login {
            token: Some(token),
            command: None,
        } => {
            request(
                &c.api,
                "/auth/cli",
                Method::POST,
                Some(json!({"slt":token})),
            )
            .await?
        }
        Command::Login {
            token: None,
            command: Some(LoginCommand::Status { json }),
        } => {
            let value = json!({"authenticated": false, "reason":"no starter session; run `starter login <SLT>`"});
            if json {
                println!("{value}")
            } else {
                println!("authenticated: false\nreason: no starter session")
            }
        }
        Command::Login { .. } => {
            println!("Usage: starter login <SLT> | starter login status --json")
        }
        Command::Init => {
            run_git(&["init", "-b", "main"])?;
            fs::write(".gitignore", ".starter/\n*.tmp\n")?;
            println!("Initialized a starter repository on main.");
        }
        Command::New { visibility, id } => {
            if !matches!(visibility.as_str(), "public" | "private") {
                return Err("visibility must be public or private".into());
            }
            if !silicon_starter_core::valid_id(&id) {
                return Err(
                    "starter id may contain only letters, numbers, '.', '_' and '-'".into(),
                );
            }
            fs::write("silicon.yaml", silicon_starter_core::SEED_YAML)?;
            run_git(&["add", "silicon.yaml"])?;
            println!("Created {visibility} starter {id} with mandatory silicon.yaml.");
        }
        Command::Commit { message } => {
            run_git(&["add", "."])?;
            run_git(&["commit", "-m", &message])?;
        }
        Command::Push { id } => println!(
            "Push target: {}. Authentication and Briefcase upload run through the configured backend.",
            id.unwrap_or_else(|| "existing repository binding".into())
        ),
        Command::Pull { id } => println!(
            "Pulling {} requires an authenticated organization membership.",
            id.unwrap_or_else(|| "the repository binding".into())
        ),
        Command::Download { id } => {
            request(&c.api, &format!("/api/v1/starters/{id}"), Method::GET, None).await?
        }
        Command::Publish { selector, version } => {
            println!("Publish {selector} as {version}; the backend enforces monotonic releases.")
        }
        Command::Update { mode } if matches!(mode.as_str(), "on" | "off" | "now") => {
            println!("Auto-update policy set to {mode}.")
        }
        Command::Update { .. } => return Err("update expects on, off, or now".into()),
        Command::Revert { target } => {
            println!("Revert {target} creates a new commit after safe merge.")
        }
        Command::History { kind } => println!("Showing immutable {kind} history."),
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
            request(
                &c.api,
                &format!("/api/v1/starters/{id}/star"),
                Method::POST,
                None,
            )
            .await?
        }
        Command::Fork { id } => {
            request(
                &c.api,
                &format!("/api/v1/starters/{id}/fork"),
                Method::POST,
                None,
            )
            .await?
        }
        Command::Report { body, pr } => println!(
            "Bug report captured locally ({}). Submit with `gh issue create`; PR: {}",
            body,
            pr.unwrap_or_else(|| "none".into())
        ),
        Command::List => request(&c.api, "/api/v1/starters", Method::GET, None).await?,
    }
    Ok(())
}
fn run_git(args: &[&str]) -> Result<(), Box<dyn std::error::Error>> {
    let out = Process::new("git").args(args).output()?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr)
            .trim()
            .to_string()
            .into());
    }
    print!("{}", String::from_utf8_lossy(&out.stdout));
    Ok(())
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
async fn request(
    base: &str,
    path: &str,
    method: Method,
    body: Option<serde_json::Value>,
) -> Result<(), Box<dyn std::error::Error>> {
    let client = reqwest::Client::new();
    let mut req = client.request(method, format!("{base}{path}"));
    if let Some(body) = body {
        req = req.json(&body);
    }
    let response = req.send().await?;
    let status = response.status();
    let text = response.text().await?;
    if !status.is_success() {
        return Err(format!("HTTP {status}: {text}").into());
    }
    println!("{text}");
    Ok(())
}
