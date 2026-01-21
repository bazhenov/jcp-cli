//! Test harness for integration testing the ACP-JCP adapter.
//!
//! Provides a clean API for testing the adapter without dealing with
//! websocket setup, channels, and async coordination directly.

use acp_jcp::{Config, RawIncomingMessage, StdinStream, StdoutSink, run_adapter};
use agent_client_protocol::{
    AgentResponse, AgentSide, ClientRequest, ClientSide, JsonRpcMessage, OutgoingMessage, Request,
    RequestId, Response, Side,
};
use futures_util::{SinkExt, StreamExt};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt, DuplexStream},
    net::TcpListener,
    sync::mpsc,
    task::JoinHandle,
    time::timeout,
};
use tokio_tungstenite::accept_async;
use tungstenite::{Message, Utf8Bytes};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(2);

/// Test harness for the ACP-JCP adapter.
///
/// Provides a clean API for sending messages from the client side,
/// receiving them on the server side, and vice versa.
pub struct AdapterTestHarness {
    /// Send messages to the mock server (which then sends to adapter)
    to_adapter_tx: mpsc::Sender<String>,
    /// Receive messages that adapter sent to mock server
    from_adapter_rx: mpsc::Receiver<String>,
    /// Write to this to simulate client stdin
    client_stdin: DuplexStream,
    /// Read from this to get client stdout
    client_stdout: DuplexStream,
    /// Handle to wait for adapter shutdown
    adapter_handle: JoinHandle<()>,
    /// Handle to wait for mock server shutdown
    server_handle: JoinHandle<()>,
    /// Next request ID for client requests
    next_request_id: u32,
}

impl AdapterTestHarness {
    /// Bootstrap a new test harness with the given config.
    pub async fn new(config: Config) -> Self {
        // Channels for test harness <-> mock server communication
        let (to_adapter_tx, to_adapter_rx) = mpsc::channel::<String>(10);
        let (from_adapter_tx, from_adapter_rx) = mpsc::channel::<String>(10);

        // Start mock websocket server
        let (addr, server_handle) = start_mock_ws_server(to_adapter_rx, from_adapter_tx).await;

        // Create client side channels (simulating stdin/stdout)
        let (client_stdin, client_stdin_rx) = tokio::io::duplex(4096);
        let (client_stdout_tx, client_stdout) = tokio::io::duplex(4096);

        // Connect adapter to mock server
        let ws_url = format!("ws://{}", addr);
        let (ws_stream, _) = tokio_tungstenite::connect_async(&ws_url).await.unwrap();
        let (server_tx, server_rx) = ws_stream.split();

        // Spawn the adapter
        let adapter_handle = tokio::spawn(async move {
            run_adapter(
                config,
                StdinStream::new(client_stdin_rx),
                StdoutSink::new(client_stdout_tx),
                server_rx,
                server_tx,
            )
            .await;
        });

        Self {
            to_adapter_tx,
            from_adapter_rx,
            client_stdin,
            client_stdout,
            adapter_handle,
            server_handle,
            next_request_id: 1,
        }
    }

    /// Send a request from the client to the adapter.
    ///
    /// This simulates a client (IDE) sending a JSON-RPC request via stdin.
    pub async fn client_send(&mut self, request: ClientRequest) -> RequestId {
        let id = RequestId::Number(self.next_request_id as i64);
        self.next_request_id += 1;

        let method = request.method().to_string();
        let msg =
            JsonRpcMessage::wrap(OutgoingMessage::Request::<ClientSide, AgentSide>(Request {
                id: id.clone(),
                method: method.into(),
                params: Some(request),
            }));

        let json = serde_json::to_string(&msg).unwrap();
        self.client_stdin
            .write_all(format!("{}\n", json).as_bytes())
            .await
            .unwrap();

        id
    }

    /// Send a raw JSON-RPC message from the client.
    ///
    /// Useful for testing edge cases or notifications.
    #[allow(dead_code)]
    pub async fn client_send_raw(&mut self, json: &str) {
        self.client_stdin
            .write_all(format!("{}\n", json).as_bytes())
            .await
            .unwrap();
    }

    /// Receive a request that the adapter forwarded to the server.
    ///
    /// Returns the raw JSON value for flexible assertions.
    pub async fn server_recv(&mut self) -> Value {
        let msg = timeout(DEFAULT_TIMEOUT, self.from_adapter_rx.recv())
            .await
            .expect("timeout waiting for server to receive message")
            .expect("channel closed");

        serde_json::from_str(&msg).expect("invalid JSON from adapter")
    }

    /// Receive a request that the adapter forwarded to the server, parsed as ClientRequest.
    pub async fn server_recv_request(&mut self) -> (RequestId, ClientRequest) {
        let value = self.server_recv().await;

        let id = match &value["id"] {
            Value::Number(n) => RequestId::Number(n.as_i64().unwrap()),
            Value::String(s) => RequestId::Str(s.clone()),
            _ => panic!("invalid request id"),
        };

        let method = value["method"].as_str().expect("missing method");
        let params = value.get("params");

        let request = AgentSide::decode_request(
            method,
            params
                .map(|p| serde_json::value::RawValue::from_string(p.to_string()).unwrap())
                .as_deref(),
        )
        .expect("failed to decode request");

        (id, request)
    }

    /// Send a response from the server back to the adapter.
    pub async fn server_reply(&mut self, id: RequestId, response: AgentResponse) {
        let msg = JsonRpcMessage::wrap(OutgoingMessage::Response::<AgentSide, ClientSide>(
            Response::new(id, Ok::<_, agent_client_protocol::Error>(response)),
        ));

        let json = serde_json::to_string(&msg).unwrap();
        self.to_adapter_tx.send(json).await.unwrap();
    }

    /// Send a raw JSON response from the server.
    #[allow(dead_code)]
    pub async fn server_reply_raw(&mut self, json: &str) {
        self.to_adapter_tx.send(json.to_string()).await.unwrap();
    }

    /// Receive a response that the adapter forwarded to the client.
    ///
    /// Returns the raw JSON value for flexible assertions.
    pub async fn client_recv<T: DeserializeOwned>(&mut self) -> Response<T> {
        let mut buf = vec![0u8; 4096];
        let n = timeout(DEFAULT_TIMEOUT, self.client_stdout.read(&mut buf))
            .await
            .expect("timeout waiting for client to receive message")
            .expect("read error");

        let response = String::from_utf8_lossy(&buf[..n]);
        serde_json::from_str(&response).expect("invalid JSON response")
    }

    /// Shutdown the test harness gracefully.
    pub async fn shutdown(self) {
        drop(self.client_stdin);
        drop(self.to_adapter_tx);
        let _ = timeout(Duration::from_millis(100), self.adapter_handle).await;
        let _ = timeout(Duration::from_millis(100), self.server_handle).await;
    }
}

/// Start a mock websocket server.
/// Returns the address and a JoinHandle for proper shutdown.
async fn start_mock_ws_server(
    to_client_rx: mpsc::Receiver<String>,
    from_client_tx: mpsc::Sender<String>,
) -> (SocketAddr, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let handle = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let ws_stream = accept_async(stream).await.unwrap();
        let (ws_tx, ws_rx) = ws_stream.split();

        // Adapt websocket to String streams/sinks
        let ws_rx_strings = ws_rx.filter_map(|msg| async move {
            match msg {
                Ok(Message::Text(text)) => Some(text.to_string()),
                _ => None,
            }
        });
        let ws_tx_strings = ws_tx.with(|s: String| async move {
            Ok::<_, tungstenite::Error>(Message::Text(Utf8Bytes::from(s)))
        });

        // Server receives from websocket and sends to test harness
        let receive_handle = tokio::spawn(async move {
            let mut ws_rx_strings = std::pin::pin!(ws_rx_strings);
            while let Some(msg) = ws_rx_strings.next().await {
                if from_client_tx.send(msg).await.is_err() {
                    break;
                }
            }
        });

        // Server receives from test harness and sends to websocket
        let send_handle = tokio::spawn(async move {
            let mut to_client_rx = to_client_rx;
            let mut ws_tx_strings = std::pin::pin!(ws_tx_strings);
            while let Some(msg) = to_client_rx.recv().await {
                if ws_tx_strings.send(msg).await.is_err() {
                    break;
                }
            }
        });

        let _ = tokio::join!(receive_handle, send_handle);
    });

    (addr, handle)
}
