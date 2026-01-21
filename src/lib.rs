use agent_client_protocol::{self as acp, RawValue};
use serde::{Deserialize, Serialize};
use std::{fs::File, io::Write};

#[derive(Serialize, Deserialize)]
#[cfg_attr(test, derive(Debug, PartialEq))]
pub struct NewSessionMeta {
    #[serde(rename = "remote")]
    pub remote: NewSessionRemote,

    #[serde(rename = "jbAiToken")]
    pub jb_ai_token: String,

    #[serde(rename = "supportsUserGitAuthFlow")]
    pub supports_user_git_auth_flow: bool,
}

#[derive(Serialize, Deserialize)]
#[cfg_attr(test, derive(Debug, PartialEq))]
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

/// Simple wrapper that writes a copy of all traffic in a log file
struct TrafficLog(Option<File>);

impl TrafficLog {
    fn log(&mut self, data: impl AsRef<[u8]>) {
        if let Some(file) = self.0.as_mut() {
            let _ = file.write_all(data.as_ref());
            let _ = file.write_all(b"\n");
        }
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
