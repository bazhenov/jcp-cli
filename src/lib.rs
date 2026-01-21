use agent_client_protocol::{
    self as acp, AgentSide, ClientRequest, ClientSide, JsonRpcMessage, NewSessionRequest,
    OutgoingMessage, RawValue, Request, Side,
};
use futures::{Sink, Stream};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fmt::Debug,
    io,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader, Lines};
use tungstenite::{Message, Utf8Bytes};

/// A Stream adapter that reads newline-delimited JSON from an async reader
pub struct StdinStream<R> {
    lines: Lines<BufReader<R>>,
}

impl<R: AsyncRead + Unpin> StdinStream<R> {
    pub fn new(reader: R) -> Self {
        Self {
            lines: BufReader::new(reader).lines(),
        }
    }
}

impl<R: AsyncRead + Unpin> Stream for StdinStream<R> {
    type Item = Result<String, io::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.lines).poll_next_line(cx).map(|result| {
            match result {
                Ok(Some(line)) => Some(Ok(line)),
                Ok(None) => None, // EOF
                Err(e) => Some(Err(e)),
            }
        })
    }
}

/// A Sink adapter that writes newline-delimited strings to an async writer
pub struct StdoutSink<W> {
    writer: W,
    buffer: Vec<u8>,
    written: usize,
}

impl<W: tokio::io::AsyncWrite + Unpin> StdoutSink<W> {
    pub fn new(writer: W) -> Self {
        Self {
            writer,
            buffer: Vec::new(),
            written: 0,
        }
    }
}

impl<W: tokio::io::AsyncWrite + Unpin> Sink<String> for StdoutSink<W> {
    type Error = io::Error;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        // Ready when buffer is empty (previous write completed)
        if self.buffer.is_empty() {
            Poll::Ready(Ok(()))
        } else {
            // Need to flush first
            self.poll_flush(cx)
        }
    }

    fn start_send(mut self: Pin<&mut Self>, item: String) -> Result<(), Self::Error> {
        // Buffer the data with newline
        self.buffer.extend_from_slice(item.as_bytes());
        self.buffer.push(b'\n');
        self.written = 0;
        Ok(())
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        let this = &mut *self;
        while this.written < this.buffer.len() {
            let buf = &this.buffer[this.written..];
            match Pin::new(&mut this.writer).poll_write(cx, buf) {
                Poll::Ready(Ok(n)) => {
                    this.written += n;
                }
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Pending => return Poll::Pending,
            }
        }
        // All data written, clear buffer
        this.buffer.clear();
        this.written = 0;
        // Flush the underlying writer
        Pin::new(&mut this.writer).poll_flush(cx)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        // Flush any remaining data first
        match self.as_mut().poll_flush(cx) {
            Poll::Ready(Ok(())) => Pin::new(&mut self.writer).poll_shutdown(cx),
            other => other,
        }
    }
}

/// Configuration for the ACP-JCP adapter
#[derive(Clone)]
pub struct Config {
    pub git_url: String,
    pub branch: String,
    pub revision: String,
    pub jb_ai_token: String,
    pub supports_user_git_auth_flow: bool,
}

impl Config {
    pub fn new_session_meta(&self) -> NewSessionMeta {
        NewSessionMeta {
            remote: NewSessionRemote {
                branch: self.branch.clone(),
                url: self.git_url.clone(),
                revision: self.revision.clone(),
            },
            jb_ai_token: self.jb_ai_token.clone(),
            supports_user_git_auth_flow: self.supports_user_git_auth_flow,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub struct NewSessionMeta {
    #[serde(rename = "remote")]
    pub remote: NewSessionRemote,

    #[serde(rename = "jbAiToken")]
    pub jb_ai_token: String,

    #[serde(rename = "supportsUserGitAuthFlow")]
    pub supports_user_git_auth_flow: bool,
}

#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub struct NewSessionRemote {
    #[serde(rename = "branch")]
    pub branch: String,

    #[serde(rename = "url")]
    pub url: String,

    #[serde(rename = "revision")]
    pub revision: String,
}

#[derive(Debug, Deserialize)]
pub struct RawIncomingMessage<'a> {
    pub id: Option<acp::RequestId>,
    pub method: Option<&'a str>,
    pub params: Option<&'a RawValue>,
    pub result: Option<&'a RawValue>,
    pub error: Option<acp::Error>,
}

/// Run the adapter connecting client streams to server streams
///
/// This function bridges:
/// - `client_rx`: incoming string messages from the client (e.g., stdin lines)
/// - `client_tx`: outgoing string messages to the client (e.g., stdout)
/// - `server_rx`: incoming messages from the websocket server
/// - `server_tx`: outgoing messages to the websocket server
///
/// Framing (newline delimiting) should be handled by the stream/sink implementations.
pub async fn run_adapter<ClientRx, ClientTx, ServerRx, ServerTx>(
    config: Config,
    client_rx: ClientRx,
    client_tx: ClientTx,
    server_rx: ServerRx,
    server_tx: ServerTx,
) where
    ClientRx: Stream<Item = Result<String, io::Error>> + Unpin + Send + 'static,
    ClientTx: Sink<String> + Unpin + Send + 'static,
    ClientTx::Error: Debug,
    ServerRx: Stream<Item = Result<Message, tungstenite::Error>> + Unpin + Send + 'static,
    ServerTx: Sink<Message> + Unpin + Send + 'static,
    ServerTx::Error: Debug,
{
    let new_session_meta = config.new_session_meta();

    let uplink = tokio::spawn(uplink_task(server_tx, client_rx, new_session_meta));
    let downlink = tokio::spawn(downlink_task(server_rx, client_tx));

    let _ = tokio::join!(uplink, downlink);
}

/// Server -> Client task: forwards messages from websocket to client
pub async fn downlink_task<ServerRx, ClientTx>(mut server_rx: ServerRx, mut client_tx: ClientTx)
where
    ServerRx: Stream<Item = Result<Message, tungstenite::Error>> + Unpin,
    ClientTx: Sink<String> + Unpin,
    ClientTx::Error: Debug,
{
    while let Some(msg) = server_rx.next().await {
        if let Ok(msg) = msg {
            let data = msg.into_text().unwrap().to_string();
            client_tx.send(data).await.unwrap();
        }
    }
}

/// Client -> Server task: forwards messages from client to websocket
pub async fn uplink_task<ServerTx, ClientRx>(
    mut server_tx: ServerTx,
    mut client_rx: ClientRx,
    new_session_meta: NewSessionMeta,
) where
    ServerTx: Sink<Message> + Unpin,
    ServerTx::Error: Debug,
    ClientRx: Stream<Item = Result<String, io::Error>> + Unpin,
{
    while let Some(Ok(msg)) = client_rx.next().await {
        let rpc_msg: RawIncomingMessage<'_> = serde_json::from_slice(msg.as_bytes()).unwrap();

        if let Some((method, id)) = rpc_msg.method.zip(rpc_msg.id) {
            let mut request = AgentSide::decode_request(method, rpc_msg.params).unwrap();

            if let ClientRequest::NewSessionRequest(r) = &mut request {
                inject_new_session_meta(r, &new_session_meta);
            }

            let msg =
                JsonRpcMessage::wrap(OutgoingMessage::Request::<ClientSide, AgentSide>(Request {
                    id,
                    method: method.into(),
                    params: Some(request),
                }));

            let json = serde_json::to_string(&msg).unwrap();

            server_tx
                .send(Message::Text(Utf8Bytes::from(&json)))
                .await
                .unwrap();
        } else {
            // Sending notifications to JCP without modification
            server_tx
                .send(Message::Text(Utf8Bytes::from(msg)))
                .await
                .unwrap();
        }
    }
}

/// JCP needs to know where to clone git repo from and what branch to use
fn inject_new_session_meta(req: &mut NewSessionRequest, meta: &NewSessionMeta) {
    if let Value::Object(json) = serde_json::to_value(meta).unwrap() {
        req.meta = Some(json);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::{AgentSide, ClientRequest, Side};
    use serde::de::DeserializeOwned;
    use serde_json::Value;
    use std::fmt::Debug;

    #[test]
    fn test_new_session_meta_deserialization() {
        check_serialization(
            r#"{
                "remote": {
                    "branch":"main",
                    "url":"https://example.com/repo.git",
                    "revision":"18adf27d36912b2e255c71327146ac21116e232f"
                },
                "jbAiToken": "test_token",
                "supportsUserGitAuthFlow": false
            }"#,
            NewSessionMeta {
                remote: NewSessionRemote {
                    branch: "main".to_string(),
                    url: "https://example.com/repo.git".to_string(),
                    revision: "18adf27d36912b2e255c71327146ac21116e232f".to_string(),
                },
                jb_ai_token: "test_token".to_string(),
                supports_user_git_auth_flow: false,
            },
        );
    }

    #[test]
    fn json_rpc_request_can_be_deserialized_using_raw_request() {
        let json = r#"{
            "jsonrpc":"2.0",
            "id":0,
            "method":"initialize",
            "params": {
                "protocolVersion":1,
                "clientCapabilities": {
                    "fs": {
                        "readTextFile":true,
                        "writeTextFile":true
                    },
                    "terminal":true,
                    "_meta": {},
                    "clientInfo": {"name":"ide","title":"IDE","version":"0.1"}
                }
            }
        }"#;

        let raw_message: RawIncomingMessage = serde_json::from_str(json).unwrap();
        let request =
            AgentSide::decode_request(raw_message.method.unwrap(), raw_message.params).unwrap();

        if !matches!(request, ClientRequest::InitializeRequest(..)) {
            panic!("Unexpected request: {:?}", request);
        }
    }

    fn check_serialization<T>(json: &str, expected_value: T)
    where
        T: DeserializeOwned + Serialize + PartialEq + Debug,
    {
        let json: Value = serde_json::from_str(json).unwrap();
        let deserialized: T = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(deserialized, expected_value);
        let serialized = serde_json::to_value(deserialized).unwrap();
        assert_eq!(json, serialized);
    }
}
