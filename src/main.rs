use acp_jcp::{NewSessionMeta, NewSessionRemote, RawIncomingMessage};
use agent_client_protocol::{self as acp, ClientRequest, IncomingMessage, Side};
use agent_client_protocol::{
    AgentSide, ClientCapabilities, ClientSide, FileSystemCapability, JsonRpcMessage,
    OutgoingMessage, ProtocolVersion, Request, RequestId,
};
use dotenv::dotenv;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::{env, io};
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

    let log_file = env::var("TRAFFIC_LOG")
        .ok()
        .and_then(|f| File::create(f).ok());
    let mut traffic_log = TrafficLog(log_file);

    let (ws_stream, _) = connect_async(request).await.unwrap();
    let (mut server_tx, mut server_rx) = ws_stream.split();

    let mut client_rx = stdout();
    let client_tx = BufReader::new(stdin());
    let mut client_tx_lines = client_tx.lines();

    loop {
        tokio::select! {
            client_to_server_msg = client_tx_lines.next_line() => {
                if let Some(msg) = client_to_server_msg.unwrap() {
                    let rpc_msg: RawIncomingMessage<'_> =
                        serde_json::from_slice(msg.as_bytes()).unwrap();

                    if let Some((method, id)) = rpc_msg.method.zip(rpc_msg.id) {
                        let mut request = AgentSide::decode_request(method, rpc_msg.params).unwrap();

                        if let ClientRequest::NewSessionRequest(mut r) = request {
                            // Inject session/new meta
                            let remote_meta = NewSessionMeta {
                                remote: NewSessionRemote {
                                    branch: "main".into(),
                                    url: git_url.clone(),
                                    revision: "main".into(),
                                },
                                jb_ai_token: antropic_key.clone(),
                                supports_user_git_auth_flow: false
                            };

                            let Value::Object(meta) = serde_json::to_value(remote_meta).unwrap() else {
                                panic!("Unexpected value");
                            };
                            r = r.meta(meta);
                            request = ClientRequest::NewSessionRequest(r);
                        }

                        let msg = JsonRpcMessage::wrap(OutgoingMessage::Request::<ClientSide, AgentSide>(Request {
                            id,
                            method: method.into(),
                            params: Some(request)
                        }));

                        let json = serde_json::to_string(&msg).unwrap();
                        traffic_log.log(&json);

                        server_tx
                            .send(Message::Text(Utf8Bytes::from(&json)))
                            .await
                            .unwrap();
                    } else {
                        // Sending notifications to an JCP without modification
                        traffic_log.log(&msg);
                        server_tx
                            .send(Message::Text(Utf8Bytes::from(msg)))
                            .await
                            .unwrap();
                    }
                } else {
                    break;
                }
            },

            server_to_client_msg = server_rx.next() => {
                if let Some(msg) = server_to_client_msg.transpose().unwrap() {
                    let data = msg.into_data();
                    traffic_log.log(&data);
                    client_rx.write_all(&data).await.unwrap();
                    // We need to add newline to frame an JSON-RPC message in stdout
                    client_rx.write_all(b"\n").await.unwrap();
                } else {
                    break;
                }
            }
        }
    }

    // let rq = acp::ClientRequest::InitializeRequest(
    //     acp::InitializeRequest::new(ProtocolVersion::V1).client_capabilities(
    //         ClientCapabilities::new()
    //             .terminal(false)
    //             .fs(FileSystemCapability::new()
    //                 .read_text_file(false)
    //                 .write_text_file(false)),
    //     ),
    // );

    // let rpc = JsonRpcMessage::wrap(OutgoingMessage::Request::<ClientSide, AgentSide>(Request {
    //     id: RequestId::Number(1),
    //     method: "initialize".into(),
    //     params: Some(rq),
    // }));

    // let string = serde_json::to_string(&rpc).unwrap();

    // let reply = server_rx.next().await.unwrap().unwrap();

    // println!("{reply}");
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
