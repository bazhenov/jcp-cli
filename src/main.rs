use acp_jcp::{
    Adapter, Config, IoTransport, TrafficLog, WebSocketTransport,
    auth::{authenticate_and_get_refresh_token, get_access_token},
    keychain::{delete_refresh_token, get_refresh_token, store_refresh_token},
};
use clap::{Parser, Subcommand};
use dotenv::dotenv;
use futures_util::StreamExt;
use std::{env, process};
use tokio::{
    io::{stdin, stdout},
    task::spawn_blocking,
};
use tokio_tungstenite::connect_async;
use tungstenite::client::IntoClientRequest;

#[derive(Parser)]
#[command(name = "acp-jcp")]
#[command(about = "ACP-JCP adapter for JetBrains Cloud Platform")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Authenticate via browser and store refresh token in keychain
    Login,

    /// Discard local refresh token
    Logout,
}

#[tokio::main]
async fn main() {
    dotenv().ok();

    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Login) => {
            // Run login in blocking context since it uses synchronous HTTP
            spawn_blocking(|| {
                eprintln!("Starting authentication...");
                match authenticate_and_get_refresh_token() {
                    Ok(refresh_token) => {
                        if let Err(e) = store_refresh_token(&refresh_token) {
                            eprintln!("Failed to store refresh token in keychain: {}", e);
                            process::exit(1);
                        }
                        eprintln!("Login successful!");
                    }
                    Err(e) => {
                        eprintln!("Login failed: {}", e);
                        process::exit(1);
                    }
                }
            })
            .await
            .unwrap();
        }
        Some(Commands::Logout) => {
            delete_refresh_token().unwrap();
            eprintln!("Logout successful!");
        }
        None => run_adapter().await,
    }
}

async fn run_adapter() {
    let git_url = env::var("GIT_URL").expect("GIT_URL env variable should be configured");
    let anthropic_key =
        env::var("ANTHROPIC_KEY").expect("ANTHROPIC_KEY env variable should be configured");
    let jcp_url = env::var("JCP_URL")
        .ok()
        .unwrap_or("wss://api.stgn.jetbrains.cloud/agent-spawner/acp".into());
    let traffic_log = TrafficLog::new(env::var("TRAFFIC_LOG").ok()).await.unwrap();

    // First check if access token is provided directly via env var
    let jba_access_token = if let Ok(access_key) = env::var("JBA_ACCESS_TOKEN") {
        access_key
    } else {
        // Try to get refresh token from keychain and upgrade it
        let Some(refresh_token) = get_refresh_token().unwrap() else {
            eprintln!("No refresh token found");
            eprintln!("Please run `acp-jcp login` to authenticate.");
            return;
        };
        spawn_blocking(move || get_access_token(&refresh_token).unwrap())
            .await
            .unwrap()
    };

    let mut request = jcp_url.into_client_request().unwrap();
    request.headers_mut().insert(
        "Authorization",
        format!("Bearer {jba_access_token}").parse().unwrap(),
    );

    let config = Config {
        git_url,
        branch: "main".into(),
        revision: "main".into(),
        jb_ai_token: anthropic_key,
        supports_user_git_auth_flow: false,
    };

    let (ws_stream, _) = connect_async(request).await.unwrap();
    let (ws_tx, ws_rx) = ws_stream.split();

    let downlink = IoTransport::new(stdin(), stdout());
    let uplink = WebSocketTransport::new(ws_rx, ws_tx);

    let mut adapter = Adapter::new(config, downlink, uplink);
    adapter.set_traffic_log(traffic_log);
    while adapter
        .handle_next_message()
        .await
        .expect("Unable to handle message")
        .is_some()
    {}
}
