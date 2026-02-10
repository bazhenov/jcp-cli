use agent_client_protocol::{
    AgentSide, ClientRequest, ClientSide, InitializeRequest, InitializeResponse, JsonRpcMessage,
    NewSessionRequest, NewSessionResponse, OutgoingMessage, ProtocolVersion, Request, RequestId,
};
use serde::de::DeserializeOwned;
use std::{
    io::{BufRead, BufReader, Write},
    path::PathBuf,
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
};
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

    // Step 1: Initialize handshale
    let response: InitializeResponse = e2e.client_send(ClientRequest::InitializeRequest(
        InitializeRequest::new(ProtocolVersion::V1),
    ));
    assert_eq!(response.protocol_version, ProtocolVersion::V1);

    // Step 2: Creating a new session
    let response: NewSessionResponse = e2e.client_send(ClientRequest::NewSessionRequest(
        NewSessionRequest::new("./"),
    ));
    assert!(!response.session_id.0.is_empty());
}

/// E2E test harness that manages mock server and jcp processes.
///
/// Spawns `agent_mock_server` and `jcp acp`, providing a typed API
/// for sending client requests and receiving responses.
struct E2eHarness {
    jcp: ChildProcess,
    mock_server: Child,
    next_request_id: i64,
}

impl E2eHarness {
    /// Spawn the mock server and jcp process, ready for testing.
    fn bootstrap() -> Self {
        let mut mock_server = Command::new(get_agent_mock_server_binary_path())
            .stdout(Stdio::piped())
            .spawn()
            .expect("Failed to start mock server");

        let stdout = mock_server.stdout.take().unwrap();
        let mut reader = BufReader::new(stdout);
        let mut url = String::new();
        reader.read_line(&mut url).unwrap();
        let url_str = url.trim().to_string();
        if let Err(e) = Url::parse(&url_str) {
            eprintln!("Invalid url: {url_str}");
            eprintln!("First line expected to be an URL: {e}");
            panic!();
        }

        let jcp = ChildProcess::spawn(
            get_jcp_binary_path(),
            &["acp"],
            &[
                ("JCP_URL", &url_str),
                ("AI_PLATFORM_TOKEN", "test-token"),
                ("JBA_ACCESS_TOKEN", "test-access-token"),
            ],
        );

        Self {
            jcp,
            mock_server,
            next_request_id: 1,
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
}

impl Drop for E2eHarness {
    fn drop(&mut self) {
        self.mock_server.kill().ok();
        self.mock_server.wait().ok();
    }
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

fn get_agent_mock_server_binary_path() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_BIN_EXE_agent_mock_server"));
    if cfg!(windows) {
        path.set_extension("exe");
    }
    path
}
