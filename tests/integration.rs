mod harness;

use acp_jcp::{Config, NewSessionMeta};
use agent_client_protocol::{
    AgentResponse, ClientRequest, InitializeRequest, InitializeResponse, NewSessionRequest,
    ProtocolVersion, Response,
};
use harness::AdapterTestHarness;

const TEST_GIT_URL: &str = "https://github.com/test/repo.git";
const TEST_BRANCH: &str = "main";
const TEST_REVISION: &str = "abc123";
const TEST_TOKEN: &str = "test-token";

fn test_config() -> Config {
    Config {
        git_url: TEST_GIT_URL.into(),
        branch: TEST_BRANCH.into(),
        revision: TEST_REVISION.into(),
        jb_ai_token: TEST_TOKEN.into(),
        supports_user_git_auth_flow: false,
    }
}

#[tokio::test]
async fn test_adapter_forwards_initialize_request_to_server() {
    let mut harness = AdapterTestHarness::new(test_config()).await;

    // Client sends initialize request
    let request = ClientRequest::InitializeRequest(InitializeRequest::new(1.into()));
    let request_id = harness.client_send(request).await;

    // Server receives the forwarded request
    let (recv_id, recv_request) = harness.server_recv_request().await;
    assert_eq!(recv_id, request_id);
    assert!(matches!(recv_request, ClientRequest::InitializeRequest(_)));

    let initalize_response = InitializeResponse::new(ProtocolVersion::V1);
    // Server sends response
    let response = AgentResponse::InitializeResponse(initalize_response.clone());
    harness.server_reply(recv_id, response).await;

    // Client receives the response
    let result = harness.client_recv::<InitializeResponse>().await;
    let Response::Result { id, result } = result else {
        panic!("expected InitializeResponse, got {:?}", result);
    };

    assert_eq!(id, request_id);
    assert_eq!(result, initalize_response);

    harness.shutdown().await;
}

#[tokio::test]
async fn test_adapter_injects_meta_into_new_session_request() {
    let config = test_config();
    let expected_meta = config.new_session_meta();
    let mut harness = AdapterTestHarness::new(config).await;

    // Client sends newSession request (without meta)
    harness
        .client_send(ClientRequest::NewSessionRequest(NewSessionRequest::new(
            "/test",
        )))
        .await;

    // Server receives the request with injected meta
    let (_, received) = harness.server_recv_request().await;
    let ClientRequest::NewSessionRequest(r) = received else {
        panic!("expected NewSessionRequest, got {:?}", received);
    };

    // Verify the meta was injected by deserializing it
    let meta = r
        .meta
        .map(|m| serde_json::from_value::<NewSessionMeta>(serde_json::Value::Object(m)))
        .transpose()
        .expect("meta should be valid");

    assert_eq!(meta, Some(expected_meta));
    harness.shutdown().await;
}
