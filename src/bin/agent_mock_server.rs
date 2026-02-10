/// A mock ACP server that handles WebSocket connections and responds to
/// InitializeRequest and PromptRequest in a naive way.
///
/// On startup, prints the WebSocket URL to stdout (e.g., "ws://127.0.0.1:12345")
/// so tests can read it and configure the client accordingly.
use agent_client_protocol::{
    AgentResponse, AgentSide, ClientRequest, ClientSide, InitializeResponse, JsonRpcMessage,
    OutgoingMessage, PromptResponse, RawValue, RequestId, Response, Side, StopReason,
};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;

use tokio::net::TcpListener;
use tokio_tungstenite::accept_async;
use tungstenite::{Message, Utf8Bytes};

#[derive(Debug, Deserialize)]
struct RawIncomingMessage<'a> {
    #[serde(rename = "id")]
    id: Option<RequestId>,

    #[serde(rename = "method")]
    method: Option<&'a str>,

    #[serde(rename = "params")]
    params: Option<&'a RawValue>,
}

#[tokio::main]
async fn main() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();

    let addr = listener.local_addr().unwrap();

    // Print the URL so the test can read it
    println!("ws://{addr}");

    let (tcp_stream, _) = listener.accept().await.unwrap();
    let ws_stream = accept_async(tcp_stream).await.unwrap();
    let (mut tx, mut rx) = ws_stream.split();

    while let Some(msg) = rx.next().await {
        let msg = msg.unwrap();

        let Message::Text(text) = msg else {
            continue;
        };

        let raw: RawIncomingMessage<'_> = serde_json::from_str(&text).unwrap();
        let Some((method, id)) = raw.method.zip(raw.id) else {
            continue;
        };

        let request = AgentSide::decode_request(method, raw.params).unwrap();
        let response = match request {
            ClientRequest::InitializeRequest(req) => {
                AgentResponse::InitializeResponse(InitializeResponse::new(req.protocol_version))
            }
            ClientRequest::PromptRequest(_) => {
                AgentResponse::PromptResponse(PromptResponse::new(StopReason::EndTurn))
            }
            _ => continue,
        };

        let msg = JsonRpcMessage::wrap(OutgoingMessage::Response::<AgentSide, ClientSide>(
            Response::new(id, Ok::<_, agent_client_protocol::Error>(response)),
        ));

        let json = serde_json::to_string(&msg).unwrap();
        tx.send(Message::Text(Utf8Bytes::from(json))).await.unwrap();
    }
}
