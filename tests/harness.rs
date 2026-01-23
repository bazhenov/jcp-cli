//! Test harness for integration testing the ACP-JCP adapter.
//!
//! Provides a clean API for testing the adapter without dealing with
//! websocket setup, channels, and async coordination directly.
//!
//! The harness drives the adapter synchronously via `step()`, eliminating
//! the need for timeouts and making tests deterministic.

use acp_jcp::{Adapter, Config, Transport};
use agent_client_protocol::{
    AgentResponse, AgentSide, ClientRequest, ClientSide, JsonRpcMessage, OutgoingMessage, Request,
    RequestId, Response, Side,
};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::io;
use tokio::sync::mpsc;

/// Test harness for the ACP-JCP adapter.
///
/// Provides a clean API for sending messages from the client side,
/// receiving them on the server side, and vice versa.
///
/// The adapter is driven synchronously via `step()`, making tests
/// deterministic without timeouts.
pub struct TestHarness {
    /// The adapter instance
    adapter: Adapter<ChannelTransport, ChannelTransport>,
    /// Transport endpoint for the client side (simulates IDE)
    client: ChannelTransport,
    /// Transport endpoint for the server side (simulates JCP)
    server: ChannelTransport,
    /// Next request ID for client requests
    next_request_id: u32,
}

impl TestHarness {
    /// Bootstrap a new test harness with the given config.
    pub fn new(config: Config) -> Self {
        let (downlink_adapter, downlink_test) = ChannelTransport::pair(10);
        let (uplink_adapter, uplink_test) = ChannelTransport::pair(10);

        let adapter = Adapter::new(config, downlink_adapter, uplink_adapter);

        Self {
            adapter,
            client: downlink_test,
            server: uplink_test,
            next_request_id: 1,
        }
    }

    /// Process the next message in the adapter.
    ///
    /// Returns `Some(())` if a message was processed, `None` if channels are closed.
    pub async fn step(&mut self) -> Option<()> {
        self.adapter.handle_next_message().await
    }

    /// Send a request from the client to the adapter.
    ///
    /// This simulates a client (IDE) sending a JSON-RPC request via stdin.
    /// Note: Call `step()` after this to process the message.
    pub async fn client_send(&mut self, request: ClientRequest) -> RequestId {
        let id = RequestId::Number(self.next_request_id as i64);
        self.next_request_id += 1;

        let msg =
            JsonRpcMessage::wrap(OutgoingMessage::Request::<ClientSide, AgentSide>(Request {
                id: id.clone(),
                method: request.method().to_string().into(),
                params: Some(request),
            }));

        let value = serde_json::to_value(&msg).unwrap();
        let _ = self.client.send(value).await;

        id
    }

    /// Send a raw JSON-RPC message from the client.
    ///
    /// Useful for testing edge cases or notifications.
    /// Note: Call `step()` after this to process the message.
    #[allow(dead_code)]
    pub async fn client_send_raw(&mut self, json: &str) {
        let value: Value = serde_json::from_str(json).unwrap();
        let _ = self.client.send(value).await;
    }

    /// Receive a request that the adapter forwarded to the server.
    ///
    /// Returns the raw JSON value for flexible assertions.
    /// Note: Call `step()` before this to ensure the message has been processed.
    pub fn server_recv(&mut self) -> Value {
        self.server
            .try_recv()
            .expect("no message available from server")
    }

    /// Receive a request that the adapter forwarded to the server, parsed as ClientRequest.
    ///
    /// Note: Call `step()` before this to ensure the message has been processed.
    pub fn server_recv_request(&mut self) -> (RequestId, ClientRequest) {
        let value = self.server_recv();

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
    ///
    /// Note: Call `step()` after this to process the response.
    pub async fn server_reply(&mut self, id: RequestId, response: AgentResponse) {
        let msg = JsonRpcMessage::wrap(OutgoingMessage::Response::<AgentSide, ClientSide>(
            Response::new(id, Ok::<_, agent_client_protocol::Error>(response)),
        ));

        let value = serde_json::to_value(&msg).unwrap();
        let _ = self.server.send(value).await;
    }

    /// Send a raw JSON response from the server.
    ///
    /// Note: Call `step()` after this to process the response.
    #[allow(dead_code)]
    pub async fn server_reply_raw(&mut self, json: &str) {
        let value: Value = serde_json::from_str(json).unwrap();
        let _ = self.server.send(value).await;
    }

    /// Receive a response that the adapter forwarded to the client.
    ///
    /// Returns the parsed response for assertions.
    /// Note: Call `step()` before this to ensure the response has been processed.
    pub fn client_recv<T: DeserializeOwned>(&mut self) -> Response<T> {
        let value = self
            .client
            .try_recv()
            .expect("no message available for client");

        serde_json::from_value(value).expect("invalid JSON response")
    }
}

/// Transport implementation using tokio mpsc channels.
///
/// Useful for testing where you need to control both ends of the transport.
pub struct ChannelTransport {
    rx: mpsc::Receiver<Value>,
    tx: mpsc::Sender<Value>,
}

impl ChannelTransport {
    pub fn new(rx: mpsc::Receiver<Value>, tx: mpsc::Sender<Value>) -> Self {
        Self { rx, tx }
    }

    /// Create a pair of connected transports.
    ///
    /// Returns `(a, b)` where messages sent on `a` are received on `b` and vice versa.
    pub fn pair(buffer: usize) -> (Self, Self) {
        let (tx_a, rx_a) = mpsc::channel(buffer);
        let (tx_b, rx_b) = mpsc::channel(buffer);
        (Self::new(rx_a, tx_b), Self::new(rx_b, tx_a))
    }

    /// Try to receive a message without blocking.
    ///
    /// Returns `Some(msg)` if a message is available, `None` otherwise.
    pub fn try_recv(&mut self) -> Option<Value> {
        self.rx.try_recv().ok()
    }
}

impl Transport for ChannelTransport {
    async fn recv(&mut self) -> io::Result<Option<Value>> {
        Ok(self.rx.recv().await)
    }

    async fn send(&mut self, msg: Value) -> io::Result<()> {
        self.tx.send(msg).await.map_err(io::Error::other)
    }
}
