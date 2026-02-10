use rexpect::{error::Error, session::PtySession, spawn};
use std::path::PathBuf;

const TIMEOUT: Option<u64> = Some(5000);

#[test]
fn help() -> Result<(), Error> {
    let mut p = spawn_jcp()?;

    p.exp_regex("Usage:")?;
    p.exp_eof()?;

    Ok(())
}

fn get_binary_path() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_BIN_EXE_jcp"));
    if cfg!(windows) {
        path.set_extension("exe");
    }
    path
}

fn spawn_jcp() -> Result<PtySession, Error> {
    let path = format!("{} help", get_binary_path().to_string_lossy());
    spawn(&path, TIMEOUT)
}
