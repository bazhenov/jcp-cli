use agent_client_protocol::{ClientRequest, InitializeRequest, InitializeResponse};
use rexpect::{
    error::Error,
    session::{PtySession, spawn_command},
    spawn,
};
use std::{
    io::BufRead,
    path::PathBuf,
    process::{Command, Stdio},
};

const TIMEOUT: Option<u64> = Some(5000);

#[test]
fn help() -> Result<(), Error> {
    let mut p = spawn_jcp(&["help"])?;

    p.exp_regex("Usage:")?;
    p.exp_eof()?;

    Ok(())
}

#[test]
fn initialize() -> Result<(), Error> {
    let mock_server_path = get_agent_mock_server_binary_path();
    let mut mock_server = Command::new(mock_server_path)
        .stdout(Stdio::piped())
        .spawn()
        .expect("Failed to start mock server");

    let stdout = mock_server.stdout.take().unwrap();
    let mut reader = std::io::BufReader::new(stdout);
    let mut url = String::new();
    reader.read_line(&mut url).unwrap();
    let url = url.trim();

    let mut p = spawn_jcp_with_env(
        &["acp"],
        &[
            ("JCP_URL", url),
            ("AI_PLATFORM_TOKEN", "test-token"),
            ("JBA_ACCESS_TOKEN", "test-access-token"),
        ],
    )?;

    let request = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1,"clientCapabilities":{}}}"#;
    p.send_line(request)?;

    let response = p.read_line()?;
    println!("{}", response);
    p.exp_regex(r#""protocolVersion""#)?;

    mock_server.kill().ok();
    mock_server.wait().ok();
    Ok(())
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

fn spawn_jcp(args: &[&str]) -> Result<PtySession, Error> {
    let cmd = format!(
        "{} {}",
        get_jcp_binary_path().to_string_lossy(),
        args.join(" ")
    );
    spawn(&cmd, TIMEOUT)
}

fn spawn_jcp_with_env(args: &[&str], env: &[(&str, &str)]) -> Result<PtySession, Error> {
    let binary = get_jcp_binary_path();

    let mut cmd = Command::new(binary);
    cmd.args(args);
    for (key, value) in env {
        cmd.env(key, value);
    }
    spawn_command(cmd, TIMEOUT)
}
