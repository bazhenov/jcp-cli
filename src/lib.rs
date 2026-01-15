use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[cfg_attr(test, derive(Debug, PartialEq))]
struct NewSessionMeta {
    #[serde(rename = "remote")]
    remote: NewSessionRemote,

    #[serde(rename = "jbAiToken")]
    jb_ai_token: String,

    #[serde(rename = "supportsUserGitAuthFlow")]
    supports_user_git_auth_flow: bool,
}

#[derive(Serialize, Deserialize)]
#[cfg_attr(test, derive(Debug, PartialEq))]
struct NewSessionRemote {
    #[serde(rename = "branch")]
    branch: String,

    #[serde(rename = "url")]
    url: String,

    #[serde(rename = "revision")]
    revision: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
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
                    revision: Some("18adf27d36912b2e255c71327146ac21116e232f".to_string()),
                },
                jb_ai_token: "test_token".to_string(),
                supports_user_git_auth_flow: false,
            },
        );
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
