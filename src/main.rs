use acp_jcp::{
    Adapter, Config, IoTransport, TrafficLog, WebSocketTransport,
    auth::{get_access_token, login},
    keychain::{delete_refresh_token, get_refresh_token, store_refresh_token},
};
use clap::{Parser, Subcommand};
use dotenv::dotenv;
use futures_util::StreamExt;
use std::{env, process};
use tokio::io::{stdin, stdout};
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
            eprintln!("Starting authentication...");
            match login().await {
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
    let ai_platform_token =
        env::var("AI_PLATFORM_TOKEN").expect("AI_PLATFORM_TOKEN env variable should be configured");
    let jcp_url = env::var("JCP_URL")
        .ok()
        .unwrap_or("wss://api.stgn.jetbrains.cloud/agent-spawner/acp".into());
    let traffic_log = TrafficLog::new(env::var("TRAFFIC_LOG").ok()).await.unwrap();

    let jba_access_token = authenticate().await;

    let mut request = jcp_url.into_client_request().unwrap();
    request.headers_mut().insert(
        "Authorization",
        format!("Bearer {jba_access_token}").parse().unwrap(),
    );

    let config = Config {
        git_url,
        branch: "main".into(),
        revision: "main".into(),
        ai_platform_token,
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

async fn authenticate() -> String {
    if let Ok(access_key) = env::var("JBA_ACCESS_TOKEN") {
        access_key
    } else {
        // Try to get refresh token from keychain and upgrade it
        let Some(refresh_token) = get_refresh_token().unwrap() else {
            eprintln!("No refresh token found");
            eprintln!("Please run `acp-jcp login` to authenticate.");
            process::exit(1);
        };
        match get_access_token(&refresh_token).await {
            Ok(token) => token,
            Err(e) => {
                eprintln!("Failed to get access token: {}", e);
                eprintln!("Please run `acp-jcp login` to re-authenticate.");
                process::exit(1);
            }
        }
    }
}
