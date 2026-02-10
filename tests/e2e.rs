use agent_client_protocol::{
    AgentSide, ClientRequest, ClientSide, InitializeRequest, InitializeResponse, JsonRpcMessage,
    OutgoingMessage, ProtocolVersion, Request,
};
use rexpect::{
    error::Error,
    session::{PtySession, spawn_command},
};
use serde::de::DeserializeOwned;
use std::{
    io::{BufRead, BufReader},
    path::PathBuf,
    process::{Child, Command, Stdio},
};
use url::Url;

const TIMEOUT: Option<u64> = Some(5000);

#[test]
fn help() -> Result<(), Error> {
    let mut p = spawn_jcp(&["help"], &[])?;

    p.exp_regex("Usage:")?;
    p.exp_eof()?;

    Ok(())
}

#[test]
fn initialize() -> Result<(), Error> {
    let mut e2e = E2eHarness::bootstrap();

    let request = ClientRequest::InitializeRequest(InitializeRequest::new(ProtocolVersion::V1));
    let response: InitializeResponse = e2e.client_send(request);

    assert_eq!(response.protocol_version, ProtocolVersion::V1);

    Ok(())
}

/// E2E test harness that manages mock server and jcp processes.
///
/// Spawns `agent_mock_server` and `jcp acp`, providing a typed API
/// for sending client requests and receiving responses.
struct E2eHarness {
    pty: PtySession,
    mock_server: Child,
    next_request_id: u32,
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
            panic!("Invalid url: {url_str}. {e}");
        }

        let pty = spawn_jcp(
            &["acp"],
            &[
                ("JCP_URL", &url_str),
                ("AI_PLATFORM_TOKEN", "test-token"),
                ("JBA_ACCESS_TOKEN", "test-access-token"),
            ],
        )
        .expect("Failed to start jcp");

        Self {
            pty,
            mock_server,
            next_request_id: 1,
        }
    }

    /// Send a typed request and receive a typed response.
    ///
    /// Serializes the request as a JSON-RPC message, sends it via stdin,
    /// reads the response line from stdout, and deserializes the `result` field.
    fn client_send<T: DeserializeOwned>(&mut self, request: ClientRequest) -> T {
        let id = agent_client_protocol::RequestId::Number(self.next_request_id as i64);
        self.next_request_id += 1;

        let msg =
            JsonRpcMessage::wrap(OutgoingMessage::Request::<ClientSide, AgentSide>(Request {
                id,
                method: request.method().to_string().into(),
                params: Some(request),
            }));

        let json = serde_json::to_string(&msg).expect("Failed to serialize request");
        self.pty.send_line(&json).expect("Failed to send request");

        // First read_line returns the echoed input from the PTY, skip it
        let r = self.pty.read_line().expect("Failed to read echo line");
        assert_eq!(json, r);

        let response_line = self.pty.read_line().expect("Failed to read response");
        let response: serde_json::Value =
            serde_json::from_str(&response_line).expect("Failed to parse response JSON");

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

fn spawn_jcp(args: &[&str], env: &[(&str, &str)]) -> Result<PtySession, Error> {
    let binary = get_jcp_binary_path();

    let mut cmd = Command::new(binary);
    cmd.args(args);
    for (key, value) in env {
        cmd.env(key, value);
    }
    spawn_command(cmd, TIMEOUT)
}
