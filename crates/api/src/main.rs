#[tokio::main]
async fn main() {
    let bind = std::env::var("STARTER_BIND").unwrap_or_else(|_| "0.0.0.0:8080".into());
    if let Err(e) = silicon_starter_api::run(&bind).await {
        eprintln!("backend stopped: {e}");
        std::process::exit(1);
    }
}
