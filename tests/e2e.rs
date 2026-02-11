use agent_client_protocol::{
    AgentResponse, AgentSide, ClientRequest, ClientSide, ContentBlock, InitializeRequest,
    InitializeResponse, JsonRpcMessage, NewSessionRequest, NewSessionResponse, OutgoingMessage,
    PromptRequest, PromptResponse, ProtocolVersion, RawValue, Request, RequestId, Response, Side,
    StopReason, TextContent,
};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use std::{
    io::{BufRead, BufReader, Write},
    net::TcpListener,
    path::PathBuf,
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    str::FromStr,
    thread::{self, JoinHandle},
};
use tungstenite::{Message, Utf8Bytes};
use url::Url;

#[test]
fn help() {
    let output = Command::new(get_jcp_binary_path())
        .arg("help")
        .output()
        .expect("Failed to run jcp help");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Usage:"), "Expected 'Usage:' in output");
}

#[test]
fn prompt_turn() {
    let mut e2e = E2eHarness::bootstrap();

    // Step 1: Initialize handshake
    let response: InitializeResponse = e2e.client_send(ClientRequest::InitializeRequest(
        InitializeRequest::new(ProtocolVersion::V1),
    ));
    assert_eq!(response.protocol_version, ProtocolVersion::V1);

    // Step 2: Creating a new session
    let response: NewSessionResponse = e2e.client_send(ClientRequest::NewSessionRequest(
        NewSessionRequest::new("./"),
    ));
    assert!(!response.session_id.0.is_empty());

    // Step 3: prompt turn
    let prompt = PromptRequest::new(
        response.session_id,
        vec![ContentBlock::Text(TextContent::new("Prompt"))],
    );
    let response: PromptResponse = e2e.client_send(ClientRequest::PromptRequest(prompt));
    assert_eq!(response.stop_reason, StopReason::EndTurn);

    e2e.shutdown();
}

/// E2E test harness that manages mock server and jcp processes.
///
/// Starts an in-process mock ACP server on a background tokio task
/// and spawns `jcp acp`, providing a typed API for sending client
/// requests and receiving responses.
struct E2eHarness {
    jcp: ChildProcess,
    next_request_id: i64,
    mock_server_handle: Option<JoinHandle<Result<(), tungstenite::Error>>>,
}

impl E2eHarness {
    /// Start the mock server and jcp process, ready for testing.
    fn bootstrap() -> Self {
        let (url, server_handle) = start_mock_server();

        let jcp = ChildProcess::spawn(
            get_jcp_binary_path(),
            &["acp"],
            &[
                ("JCP_URL", url.as_str()),
                ("AI_PLATFORM_TOKEN", "test-token"),
                ("JBA_ACCESS_TOKEN", "test-access-token"),
            ],
        );

        Self {
            jcp,
            next_request_id: 1,
            mock_server_handle: Some(server_handle),
        }
    }

    /// Send a typed request and receive a typed response.
    ///
    /// Serializes the request as a JSON-RPC message, sends it via stdin,
    /// reads the response line from stdout, and deserializes the `result` field.
    fn client_send<T: DeserializeOwned>(&mut self, request: ClientRequest) -> T {
        let id = self.next_request_id;
        self.next_request_id += 1;

        let msg =
            JsonRpcMessage::wrap(OutgoingMessage::Request::<ClientSide, AgentSide>(Request {
                id: RequestId::Number(id),
                method: request.method().to_string().into(),
                params: Some(request),
            }));

        let json = serde_json::to_string(&msg).expect("Failed to serialize request");
        self.jcp.send_line(&json);

        let response_line = self.jcp.read_line();
        let response: serde_json::Value =
            serde_json::from_str(&response_line).expect("Failed to parse response JSON");

        let request_id: i64 =
            serde_json::from_value(response["id"].clone()).expect("Unable to read id");
        assert_eq!(
            request_id, id,
            "Incoming response is expected to have id {id}, got {request_id} instead"
        );
        serde_json::from_value(response["result"].clone())
            .expect("Failed to deserialize response result")
    }

    fn shutdown(mut self) {
        if let Some(server_join_handle) = self.mock_server_handle.take() {
            self.jcp.child.kill().ok();
            self.jcp.child.wait().ok();
            server_join_handle.join().ok();
        }
    }
}

/// It's preferential to use [`Self::shutdown()`]
impl Drop for E2eHarness {
    fn drop(&mut self) {
        if let Some(server_join_handle) = self.mock_server_handle.take() {
            self.jcp.child.kill().ok();
            self.jcp.child.wait().ok();
            server_join_handle.join().ok();
        }
    }
}

#[derive(Debug, Deserialize)]
struct RawIncomingMessage<'a> {
    #[serde(rename = "id")]
    id: Option<RequestId>,

    #[serde(rename = "method")]
    method: Option<&'a str>,

    #[serde(rename = "params")]
    params: Option<&'a RawValue>,
}

/// Start a mock ACP server on a random port.
fn start_mock_server() -> (Url, JoinHandle<Result<(), tungstenite::Error>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("Failed to bind mock server");
    let addr = listener.local_addr().unwrap();
    let url = Url::from_str(&format!("ws://{addr}")).unwrap();

    let join_handle = thread::spawn(move || {
        let (tcp_stream, _) = listener.accept().unwrap();
        let mut ws = tungstenite::accept(tcp_stream).unwrap();

        loop {
            let msg = match ws.read() {
                Ok(msg) => msg,
                Err(tungstenite::Error::ConnectionClosed) => return Ok(()),
                Err(e) => return Err(e),
            };

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
                ClientRequest::NewSessionRequest(_) => {
                    AgentResponse::NewSessionResponse(NewSessionResponse::new("1"))
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
            ws.send(Message::Text(Utf8Bytes::from(json))).unwrap();
        }
    });

    (url, join_handle)
}

/// A simple wrapper around a child process with piped stdin/stdout.
struct ChildProcess {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl ChildProcess {
    fn spawn(program: PathBuf, args: &[&str], env: &[(&str, &str)]) -> Self {
        let mut cmd = Command::new(program);
        cmd.args(args).stdin(Stdio::piped()).stdout(Stdio::piped());
        for (key, value) in env {
            cmd.env(key, value);
        }

        let mut child = cmd.spawn().expect("Failed to spawn child process");
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());

        Self {
            child,
            stdin,
            stdout,
        }
    }

    fn send_line(&mut self, line: &str) {
        // It's important to send newline character, so that transport will trigger on a new message
        writeln!(self.stdin, "{}", line).expect("Failed to write to child stdin");
        self.stdin.flush().expect("Failed to flush child stdin");
    }

    fn read_line(&mut self) -> String {
        let mut line = String::new();
        self.stdout
            .read_line(&mut line)
            .expect("Failed to read from child stdout");
        line
    }
}

impl Drop for ChildProcess {
    fn drop(&mut self) {
        self.child.kill().ok();
        self.child.wait().ok();
    }
}

fn get_jcp_binary_path() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_BIN_EXE_jcp"));
    if cfg!(windows) {
        path.set_extension("exe");
    }
    path
}
