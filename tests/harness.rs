//! Test harness for integration testing the ACP-JCP adapter.
//!
//! Provides a clean API for testing the adapter without dealing with
//! websocket setup, channels, and async coordination directly.

use acp_jcp::{Config, run_adapter};
use agent_client_protocol::{
    AgentResponse, AgentSide, ClientRequest, ClientSide, JsonRpcMessage, OutgoingMessage, Request,
    RequestId, Response, Side,
};
use futures_util::{Sink, StreamExt};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::time::Duration;
use std::{
    io,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::{sync::mpsc, task::JoinHandle, time::timeout};
use tokio_stream::wrappers::ReceiverStream;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(2);

/// A Sink wrapper for mpsc::Sender<String>
struct MpscSink {
    tx: mpsc::Sender<String>,
}

impl Sink<String> for MpscSink {
    type Error = mpsc::error::SendError<String>;

    fn poll_ready(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        // For bounded channels with enough capacity, we assume ready
        Poll::Ready(Ok(()))
    }

    fn start_send(self: Pin<&mut Self>, item: String) -> Result<(), Self::Error> {
        self.get_mut().tx.try_send(item).map_err(|e| match e {
            mpsc::error::TrySendError::Full(s) | mpsc::error::TrySendError::Closed(s) => {
                mpsc::error::SendError(s)
            }
        })
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }
}

/// Test harness for the ACP-JCP adapter.
///
/// Provides a clean API for sending messages from the client side,
/// receiving them on the server side, and vice versa.
pub struct AdapterTestHarness {
    /// Send messages to adapter (from server side)
    to_adapter_server_tx: mpsc::Sender<String>,
    /// Receive messages from adapter (server side)
    from_adapter_server_rx: mpsc::Receiver<String>,
    /// Send messages to adapter (from client side, simulating stdin)
    to_adapter_client_tx: mpsc::Sender<String>,
    /// Receive messages from adapter (client side, simulating stdout)
    from_adapter_client_rx: mpsc::Receiver<String>,
    /// Handle to wait for adapter shutdown
    adapter_handle: JoinHandle<()>,
    /// Next request ID for client requests
    next_request_id: u32,
}

impl AdapterTestHarness {
    /// Bootstrap a new test harness with the given config.
    pub async fn new(config: Config) -> Self {
        // Channels for test harness <-> adapter server side communication
        let (to_adapter_server_tx, to_adapter_server_rx) = mpsc::channel::<String>(10);
        let (from_adapter_server_tx, from_adapter_server_rx) = mpsc::channel::<String>(10);

        // Channels for test harness <-> adapter client side communication
        let (to_adapter_client_tx, to_adapter_client_rx) = mpsc::channel::<String>(10);
        let (from_adapter_client_tx, from_adapter_client_rx) = mpsc::channel::<String>(10);

        // Convert mpsc channels to Stream/Sink for the adapter
        let client_rx = ReceiverStream::new(to_adapter_client_rx).map(Ok::<_, io::Error>);
        let client_tx = MpscSink {
            tx: from_adapter_client_tx,
        };

        let server_rx = ReceiverStream::new(to_adapter_server_rx).map(Ok::<_, io::Error>);
        let server_tx = MpscSink {
            tx: from_adapter_server_tx,
        };

        // Spawn the adapter
        let adapter_handle = tokio::spawn(async move {
            run_adapter(config, client_rx, client_tx, server_rx, server_tx).await;
        });

        Self {
            to_adapter_server_tx,
            from_adapter_server_rx,
            to_adapter_client_tx,
            from_adapter_client_rx,
            adapter_handle,
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
        self.to_adapter_client_tx.send(json).await.unwrap();

        id
    }

    /// Send a raw JSON-RPC message from the client.
    ///
    /// Useful for testing edge cases or notifications.
    #[allow(dead_code)]
    pub async fn client_send_raw(&mut self, json: &str) {
        self.to_adapter_client_tx
            .send(json.to_string())
            .await
            .unwrap();
    }

    /// Receive a request that the adapter forwarded to the server.
    ///
    /// Returns the raw JSON value for flexible assertions.
    pub async fn server_recv(&mut self) -> Value {
        let msg = timeout(DEFAULT_TIMEOUT, self.from_adapter_server_rx.recv())
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
        self.to_adapter_server_tx.send(json).await.unwrap();
    }

    /// Send a raw JSON response from the server.
    #[allow(dead_code)]
    pub async fn server_reply_raw(&mut self, json: &str) {
        self.to_adapter_server_tx
            .send(json.to_string())
            .await
            .unwrap();
    }

    /// Receive a response that the adapter forwarded to the client.
    ///
    /// Returns the parsed response for assertions.
    pub async fn client_recv<T: DeserializeOwned>(&mut self) -> Response<T> {
        let msg = timeout(DEFAULT_TIMEOUT, self.from_adapter_client_rx.recv())
            .await
            .expect("timeout waiting for client to receive message")
            .expect("channel closed");

        serde_json::from_str(&msg).expect("invalid JSON response")
    }

    /// Shutdown the test harness gracefully.
    pub async fn shutdown(self) {
        drop(self.to_adapter_client_tx);
        drop(self.to_adapter_server_tx);
        let _ = timeout(Duration::from_millis(100), self.adapter_handle).await;
    }
}
