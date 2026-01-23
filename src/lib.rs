use agent_client_protocol::{
    self as acp, AgentSide, ClientRequest, ClientSide, JsonRpcMessage, NewSessionRequest,
    OutgoingMessage, RawValue, Request, Side,
};
use futures::{Sink, Stream};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader, Lines};
use tungstenite::{Message, Utf8Bytes};

/// A bidirectional transport for JSON-RPC messages.
///
/// This trait abstracts over different transport mechanisms (stdio, websocket, channels)
/// providing a simple async interface for sending and receiving JSON values.
#[allow(async_fn_in_trait)]
pub trait Transport {
    /// Receive the next message from the transport.
    /// Returns `Ok(None)` when the transport is closed.
    ///
    /// All implementations need to by cancel safe
    async fn recv(&mut self) -> io::Result<Option<Value>>;

    /// Send a message through the transport.
    async fn send(&mut self, msg: Value) -> io::Result<()>;
}

/// Transport implementation for arbitrary async readers/writers.
///
/// Reads newline-delimited JSON from reader and writes newline-delimited JSON to writer.
pub struct IoTransport {
    lines: Lines<BufReader<Box<dyn AsyncRead + Unpin + Send>>>,
    writer: Box<dyn AsyncWrite + Unpin + Send>,
}

impl IoTransport {
    pub fn new(
        reader: impl AsyncRead + Unpin + Send + 'static,
        writer: impl AsyncWrite + Unpin + Send + 'static,
    ) -> Self {
        let reader: Box<dyn AsyncRead + Unpin + Send> = Box::new(reader);
        Self {
            lines: BufReader::new(reader).lines(),
            writer: Box::new(writer),
        }
    }
}

impl Transport for IoTransport {
    async fn recv(&mut self) -> io::Result<Option<Value>> {
        // Lines::next_line() is cancellation safe per tokio documentation
        match self.lines.next_line().await? {
            Some(line) => {
                println!("Line: {line}");
                serde_json::from_str(&line)
                    .map_err(to_io_invalid_data_err)
                    .map(Some)
            }
            None => Ok(None),
        }
    }

    async fn send(&mut self, msg: Value) -> io::Result<()> {
        let json = serde_json::to_string(&msg)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        self.writer.write_all(json.as_bytes()).await?;
        self.writer.write_all(b"\n").await?;
        self.writer.flush().await
    }
}

/// Transport implementation for WebSocket connections.
pub struct WebSocketTransport {
    rx: Box<dyn Stream<Item = Result<Message, tungstenite::Error>> + Unpin + Send>,
    tx: Box<dyn Sink<Message, Error = tungstenite::Error> + Unpin + Send>,
}

impl WebSocketTransport {
    pub fn new(
        rx: impl Stream<Item = Result<Message, tungstenite::Error>> + Unpin + Send + 'static,
        tx: impl Sink<Message, Error = tungstenite::Error> + Unpin + Send + 'static,
    ) -> Self {
        Self {
            rx: Box::new(rx),
            tx: Box::new(tx),
        }
    }
}

impl Transport for WebSocketTransport {
    async fn recv(&mut self) -> io::Result<Option<Value>> {
        match self.rx.next().await {
            Some(Ok(Message::Text(text))) => serde_json::from_str(&text)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
                .map(Some),
            Some(Ok(_)) => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Only text messages are supported",
            )),
            Some(Err(e)) => Err(io::Error::other(e)),
            None => Ok(None),
        }
    }

    async fn send(&mut self, msg: Value) -> io::Result<()> {
        let message = serde_json::to_string(&msg)
            .map_err(to_io_invalid_data_err)
            .map(Utf8Bytes::from)
            .map(Message::Text)?;
        self.tx.send(message).await.map_err(io::Error::other)
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

/// Adapter that bridges ACP client and JCP server communication.
///
/// This struct processes messages from both channels using `tokio::select!`,
/// allowing for synchronous test-driving without spawning separate tasks.
pub struct Adapter<Downlink, Uplink> {
    new_session_meta: NewSessionMeta,
    downlink: Downlink,
    uplink: Uplink,
}

impl<Downlink, Uplink> Adapter<Downlink, Uplink>
where
    Downlink: Transport,
    Uplink: Transport,
{
    /// Create a new adapter with the given configuration and transports.
    ///
    /// - `downlink`: transport to the client (IDE)
    /// - `uplink`: transport to the server (JCP)
    pub fn new(config: Config, downlink: Downlink, uplink: Uplink) -> Self {
        Self {
            new_session_meta: config.new_session_meta(),
            downlink,
            uplink,
        }
    }

    /// Process the next message from either the client or server channel.
    ///
    /// Returns `Some(())` when a message was processed successfully.
    /// Returns `None` when both channels are closed (end of communication).
    pub async fn handle_next_message(&mut self) -> io::Result<Option<()>> {
        tokio::select! {
            Ok(Some(msg)) = self.downlink.recv() => {
                self.handle_client_message(msg).await?;
            }
            Ok(Some(msg)) = self.uplink.recv() => {
                self.downlink.send(msg).await?;
            }
            else => return Ok(None),
        }
        Ok(Some(()))
    }

    /// Handle a message from the client (uplink: client -> server)
    async fn handle_client_message(&mut self, msg: Value) -> io::Result<()> {
        // This is ungly hack, but we need to serialize here back to string, otherwise
        // we can not use AgentSide::decode_request()
        let msg_str = msg.to_string();
        let rpc_msg: RawIncomingMessage<'_> =
            serde_json::from_str(&msg_str).map_err(to_io_invalid_data_err)?;

        if let Some((method, id)) = rpc_msg.method.zip(rpc_msg.id) {
            let mut request = AgentSide::decode_request(method, rpc_msg.params)
                .map_err(to_io_invalid_data_err)?;

            if let ClientRequest::NewSessionRequest(r) = &mut request {
                inject_new_session_meta(r, &self.new_session_meta)?;
            }

            let msg =
                JsonRpcMessage::wrap(OutgoingMessage::Request::<ClientSide, AgentSide>(Request {
                    id,
                    method: method.into(),
                    params: Some(request),
                }));

            let value = serde_json::to_value(&msg).map_err(to_io_invalid_data_err)?;
            self.uplink.send(value).await?;
        } else {
            // Sending notifications to JCP without modification
            self.uplink.send(msg).await?;
        }
        Ok(())
    }

    /// Run the adapter until both channels are closed.
    pub async fn run(&mut self) -> io::Result<()> {
        while self.handle_next_message().await?.is_some() {}
        Ok(())
    }
}

/// JCP needs to know where to clone git repo from and what branch to use
fn inject_new_session_meta(req: &mut NewSessionRequest, meta: &NewSessionMeta) -> io::Result<()> {
    if let Value::Object(json) = serde_json::to_value(meta).map_err(to_io_invalid_data_err)? {
        req.meta = Some(json);
    }
    Ok(())
}

fn to_io_invalid_data_err<E: Into<Box<dyn std::error::Error + Send + Sync>>>(e: E) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, e)
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::{AgentSide, ClientRequest, Side};
    use drop_check::{IntersperceExt, cancelations};
    use serde::de::DeserializeOwned;
    use serde_json::Value;
    use std::fmt::Debug;
    use std::io::Cursor;

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

    #[test]
    fn io_transport_recv_is_cancel_safe() {
        use drop_check::BoxFuture;

        let json_line = b"{\"key\":\"value\"}\n";
        let expected: Value = serde_json::from_slice(json_line).unwrap();

        let init = || {
            let reader = Cursor::new(json_line.to_vec()).intersperse_pending();
            IoTransport::new(reader, vec![])
        };

        fn recv(transport: &mut IoTransport) -> BoxFuture<'_, io::Result<Option<Value>>> {
            Box::pin(transport.recv())
        }

        for (_, result) in cancelations(init, recv) {
            assert_eq!(result.unwrap(), Some(expected.clone()));
        }
    }
}
