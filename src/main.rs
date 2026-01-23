use acp_jcp::{Adapter, Config, IoTransport, WebSocketTransport};
use dotenv::dotenv;
use futures_util::StreamExt;
use std::env;
use tokio::io::{stdin, stdout};
use tokio_tungstenite::connect_async;
use tungstenite::client::IntoClientRequest;

#[tokio::main]
async fn main() {
    dotenv().ok();

    let jba_access_token =
        env::var("JBA_ACCESS_TOKEN").expect("JBA_ACCESS_TOKEN env variable should be configured");
    let git_url = env::var("GIT_URL").expect("GIT_URL env variable should be configured");

    let anthropic_key =
        env::var("ANTHROPIC_KEY").expect("ANTHROPIC_KEY env variable should be configured");

    let jcp_url = env::var("JCP_URL")
        .ok()
        .unwrap_or("wss://api.stgn.jetbrainscloud.com/agent-spawner/acp".into());

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
    while adapter.handle_next_message().await.is_some() {}
}
