use acp_jcp::{NewSessionMeta, NewSessionRemote, RawIncomingMessage};
use agent_client_protocol::{
    AgentSide, ClientRequest, ClientSide, JsonRpcMessage, NewSessionRequest, OutgoingMessage,
    Request, RequestId, Side,
};
use dotenv::dotenv;
use futures::{Sink, Stream};
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use std::env;
use std::fmt::Debug;
use std::{fs::File, io::Write};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, stdin, stdout};
use tokio_tungstenite::connect_async;
use tungstenite::{Message, Utf8Bytes, client::IntoClientRequest};

#[tokio::main]
async fn main() {
    dotenv().ok();

    let jba_access_token =
        env::var("JBA_ACCESS_TOKEN").expect("JBA_ACCESS_TOKEN env variable should be configured");
    let git_url = env::var("GIT_URL").expect("GIT_URL env variable should be configured");

    let antropic_key =
        env::var("ANTHROPIC_KEY").expect("ANTHROPIC_KEY env variable should be configured");

    let jcp_url = env::var("JCP_URL")
        .ok()
        .unwrap_or("wss://api.stgn.jetbrainscloud.com/agent-spawner/acp".into());

    let mut request = jcp_url.into_client_request().unwrap();
    request.headers_mut().insert(
        "Authorization",
        format!("Bearer {jba_access_token}").parse().unwrap(),
    );

    let new_session_request_meta = NewSessionMeta {
        remote: NewSessionRemote {
            branch: "main".into(),
            url: git_url.clone(),
            revision: "main".into(),
        },
        jb_ai_token: antropic_key.clone(),
        supports_user_git_auth_flow: false,
    };

    let (ws_stream, _) = connect_async(request).await.unwrap();
    let (server_tx, server_rx) = ws_stream.split();

    // Uplink task: Client (IDE) -> Server
    let uplink_task = tokio::spawn(uplink_task(server_tx, new_session_request_meta));
    let downlink_task = tokio::spawn(downlink_task(server_rx));

    let _ = tokio::join!(uplink_task, downlink_task);
}

async fn downlink_task<S: Stream<Item = Result<Message, tungstenite::Error>> + Unpin>(
    mut server_rx: S,
) {
    let mut client_rx = stdout();
    loop {
        let server_to_client_msg = server_rx.next().await;
        if let Some(msg) = server_to_client_msg.transpose().unwrap() {
            let data = msg.into_data();
            client_rx.write_all(&data).await.unwrap();
            // We need to add newline to frame an JSON-RPC message in stdout
            client_rx.write_all(b"\n").await.unwrap();
        } else {
            break;
        }
    }
}

async fn uplink_task<S: Sink<Message> + Unpin>(mut server_tx: S, new_session_meta: NewSessionMeta)
where
    S::Error: Debug,
{
    let rx = BufReader::new(stdin());
    let mut rx_lines = rx.lines();
    loop {
        if let Some(msg) = rx_lines.next_line().await.unwrap() {
            let rpc_msg: RawIncomingMessage<'_> = serde_json::from_slice(msg.as_bytes()).unwrap();

            if let Some((method, id)) = rpc_msg.method.zip(rpc_msg.id) {
                let mut request = AgentSide::decode_request(method, rpc_msg.params).unwrap();

                if let ClientRequest::NewSessionRequest(r) = &mut request {
                    inject_new_session_meta(r, &new_session_meta);
                }

                let json = to_json_rpc(method, id, request).unwrap();
                server_tx
                    .send(Message::Text(Utf8Bytes::from(&json)))
                    .await
                    .unwrap();
            } else {
                // Sending notifications to an JCP without modification
                server_tx
                    .send(Message::Text(Utf8Bytes::from(msg)))
                    .await
                    .unwrap();
            }
        } else {
            return;
        }
    }
}

fn to_json_rpc(
    method: &str,
    id: RequestId,
    params: ClientRequest,
) -> serde_json::error::Result<String> {
    let msg = JsonRpcMessage::wrap(OutgoingMessage::Request::<ClientSide, AgentSide>(Request {
        id,
        method: method.into(),
        params: Some(params),
    }));

    serde_json::to_string(&msg)
}

/// JCP needs to know where clone git repo from nad what branch to use
fn inject_new_session_meta(req: &mut NewSessionRequest, meta: &NewSessionMeta) {
    if let Value::Object(json) = serde_json::to_value(meta).unwrap() {
        req.meta = Some(json);
    }
}

/// Simple wrapper that writes a copy of all traffic in a log file
struct TrafficLog(Option<File>);

impl TrafficLog {
    fn log(&mut self, data: impl AsRef<[u8]>) {
        if let Some(file) = self.0.as_mut() {
            let _ = file.write_all(data.as_ref());
            let _ = file.write_all(b"\n");
        }
    }
}
