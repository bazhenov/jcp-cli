use agent_client_protocol as acp;
use agent_client_protocol::{
    AgentSide, ClientCapabilities, ClientSide, FileSystemCapability, InitializeRequest,
    JsonRpcMessage, OutgoingMessage, ProtocolVersion, Request, RequestId,
};
use dotenv::dotenv;
use futures_util::{FutureExt, SinkExt, StreamExt};
use std::env;
use tokio::io::{AsyncBufReadExt, BufReader, stdin, stdout};
use tokio_tungstenite::connect_async;
use tungstenite::{Message, Utf8Bytes, client::IntoClientRequest};

#[tokio::main]
async fn main() {
    dotenv().ok();

    let jba_access_token =
        env::var("JBA_ACCESS_TOKEN").expect("JBA_ACCESS_TOKEN env variable should be configured");

    let client_tx = stdin();
    let client_rx = stdout();

    let mut request = "wss://api.stgn.jetbrainscloud.com/agent-spawner/acp"
        .into_client_request()
        .unwrap();
    request.headers_mut().insert(
        "Authorization",
        format!("Bearer {jba_access_token}").parse().unwrap(),
    );

    let (ws_stream, _) = connect_async(request).await.unwrap();

    let (mut write, mut read) = ws_stream.split();

    let a = read.next().await.unwrap().unwrap();

    let r = BufReader::new(client_tx);
    let mut lines = r.lines();

    tokio::select! {
        client_to_server_msg = lines.next_line() => {

        }
    }

    let rq = acp::ClientRequest::InitializeRequest(
        acp::InitializeRequest::new(ProtocolVersion::V1).client_capabilities(
            ClientCapabilities::new()
                .terminal(false)
                .fs(FileSystemCapability::new()
                    .read_text_file(false)
                    .write_text_file(false)),
        ),
    );

    let rpc = JsonRpcMessage::wrap(OutgoingMessage::Request::<ClientSide, AgentSide>(Request {
        id: RequestId::Number(1),
        method: "initialize".into(),
        params: Some(rq),
    }));

    let string = serde_json::to_string(&rpc).unwrap();
    write
        .send(Message::Text(Utf8Bytes::from(&string)))
        .await
        .unwrap();

    let reply = read.next().await.unwrap().unwrap();

    println!("{reply}");
}
