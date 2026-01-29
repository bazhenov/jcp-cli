use jwt::Token;
use oauth2::{
    AuthUrl, AuthorizationCode, ClientId, ClientSecret, CsrfToken, PkceCodeChallenge, RedirectUrl,
    Scope, TokenResponse, TokenUrl, basic::BasicClient,
};
use reqwest::redirect::Policy;
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::Value;
use std::{collections::BTreeMap, error::Error};
use tiny_http::{Response, Server};
use url::Url;

/// The expected callback path for OAuth redirect
const CALLBACK_PATH: &str = "/space/auth";

pub fn authenticate_get_token() -> Result<String, Box<dyn Error + Send + Sync>> {
    // Load configuration
    let base_url = "https://public.aip.oauth.intservices.aws.intellij.net";
    let client_id = "air";
    let client_secret: Option<String> = None;
    let auth_url = format!("{}/oauth2/auth", base_url);
    let token_url = format!("{}/oauth2/token", base_url);

    // Start HTTP server on a random available port
    let server = Server::http("localhost:0")?;
    let local_addr = server
        .server_addr()
        .to_ip()
        .ok_or("Failed to get server address")?;
    let local_port = local_addr.port();

    let redirect_url = format!("http://localhost:{}{}", local_port, CALLBACK_PATH);

    // Create the OAuth2 client
    let mut client = BasicClient::new(ClientId::new(client_id.to_string()))
        .set_auth_uri(AuthUrl::new(auth_url)?)
        .set_token_uri(TokenUrl::new(token_url.clone())?)
        .set_redirect_uri(RedirectUrl::new(redirect_url)?);

    if let Some(secret) = &client_secret {
        client = client.set_client_secret(ClientSecret::new(secret.clone()));
    }

    // Generate PKCE challenge (recommended for all clients, required for public clients)
    let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();

    // Generate the authorization URL
    let (auth_url, csrf_token) = client
        .authorize_url(CsrfToken::new_random)
        .add_scope(Scope::new("offline_access".to_string())) // for retrieving refresh_token
        .add_scope(Scope::new("openid".to_string()))
        .add_scope(Scope::new("org-service".to_string()))
        .add_scope(Scope::new("jba".to_string()))
        .set_pkce_challenge(pkce_challenge)
        .url();

    eprintln!(
        "Opening browser for authentication... If the browser doesn't open, visit this URL manually:"
    );
    eprintln!("\n  {}\n", auth_url);

    // Try to open the browser
    open::that(auth_url.to_string())?;

    // Wait for the valid OAuth callback
    let code = read_authorization_code_from_callback(server, csrf_token)?;

    // Exchange the authorization code for tokens
    let http_client = reqwest::blocking::ClientBuilder::new()
        .redirect(Policy::none()) // Disable redirects to prevent SSRF vulnerabilities
        .build()?;

    let jba_token = client
        .exchange_code(code)
        .set_pkce_verifier(pkce_verifier)
        .request(&http_client)?;

    let jba_access_token = jba_token.access_token().secret();

    eprintln!("Token: {:?}", jba_access_token);
    let token: Token<Value, BTreeMap<String, Value>, _> =
        Token::parse_unverified(&jba_access_token)?;

    eprintln!("Claims: {:?}", token.claims()["aud"]);

    let jcp_url = "https://api.stgn.jetbrainscloud.com";
    let jcp_response = http_client
        .get(format!("{}/org/orgsuserinfo", jcp_url))
        .bearer_auth(jba_access_token)
        .header("Accept", "application/jwt")
        .send()?;

    if jcp_response.status() != 200 {
        return Err(format!(
            "Non 200 response from JBA: {} - {}",
            jcp_response.status(),
            jcp_response.text()?
        )
        .into());
    }

    let jcp_raw_token = jcp_response.text()?;
    eprintln!("---");
    eprintln!("{}", &jcp_raw_token);
    eprintln!("---");
    let org_token: Token<Value, JcpToken, _> = Token::parse_unverified(&jcp_raw_token)?;
    let org_token = org_token.claims();

    eprintln!("Org Token: {}", jcp_raw_token);
    eprintln!("Org Claim: {:?}", org_token);

    let org_id = &org_token
        .orgs
        .first()
        .expect("At least one org expected")
        .id;
    let response = http_client
        .post(token_url)
        //        .basic_auth(client_id, client_secret)
        //        .bearer_auth(jba_access_token)
        .form(&[
            ("grant_type", "switch_audience"),
            ("refresh_token", jba_token.refresh_token().unwrap().secret()),
            ("client_id", client_id),
            ("audience", "jcp-agent-spawner"),
            ("org_id", org_id),
            ("orgs_user_info", jcp_raw_token.as_str()),
        ])
        .send()?;

    if response.status() != 200 {
        return Err(format!(
            "Non 200 response when switching JBA audience: {} - {}",
            response.status(),
            response.text()?
        )
        .into());
    }

    let token_result = response.json::<OAuthToken>()?;

    eprintln!("TOKEN: {}", token_result.accessToken);

    //    eprintln!("SWA Token: {}", token_result.accessToken);

    Ok(token_result.accessToken)
}

#[derive(Deserialize)]
struct OAuthToken {
    #[serde(rename = "access_token")]
    accessToken: String,
}

#[derive(Deserialize, Debug)]
struct JcpToken {
    #[serde(rename = "aud")]
    audience: Vec<String>,

    #[serde(rename = "orgMemberships")]
    orgs: Vec<Organisation>,
}

#[derive(Deserialize, Debug)]
struct Organisation {
    #[serde(rename = "orgId")]
    id: String,
}

fn read_authorization_code_from_callback(
    server: Server,
    csrf_token: CsrfToken,
) -> Result<AuthorizationCode, Box<dyn Error + Send + Sync>> {
    loop {
        let Ok(request) = server.recv() else {
            continue;
        };

        let url_str = request.url();

        // Parse the URL to extract query parameters
        let full_url = format!("http://localhost{}", url_str);
        let parsed_url = match Url::parse(&full_url) {
            Ok(url) => url,
            Err(e) => {
                let description = format!("Failed to parse callback URL: {}", e);
                let response = Response::from_string(description).with_status_code(400);
                let _ = request.respond(response);
                continue;
            }
        };

        // Check if this is the expected callback path
        if parsed_url.path() != CALLBACK_PATH {
            let response = Response::from_string("Not Found").with_status_code(404);
            let _ = request.respond(response);
            continue;
        }

        // Extract authorization code
        let Some(code) = read_authorization_code(&parsed_url) else {
            // Check if there's an error parameter
            if let Some(error) = read_get_param(&parsed_url, "error") {
                let error_desc =
                    read_get_param(&parsed_url, "error_description").unwrap_or_default();

                let error_msg = format!("OAuth error: {} - {}", error, error_desc);
                let response = Response::from_string(&error_msg).with_status_code(400);
                let _ = request.respond(response);
                return Err(error_msg.into());
            }

            let response = Response::from_string("Bad Request: missing code").with_status_code(400);
            let _ = request.respond(response);
            continue;
        };

        // Extract state parameter
        let Some(state) = read_get_param(&parsed_url, "state").map(CsrfToken::new) else {
            let response =
                Response::from_string("Bad Request: missing state").with_status_code(400);
            let _ = request.respond(response);
            continue;
        };

        // Verify CSRF token
        if state.secret() != csrf_token.secret() {
            let response =
                Response::from_string("Bad Request: invalid state").with_status_code(400);
            let _ = request.respond(response);
            continue;
        }

        // Send success response to browser
        let response = Response::from_string(
            "<html><body>\
             <h1>Authentication Successful!</h1>\
             <p>You can close this window and return to the terminal.</p>\
             </body></html>",
        )
        .with_header(
            tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"text/html"[..]).unwrap(),
        );
        let _ = request.respond(response);

        return Ok(code);
    }
}

fn read_get_param(parsed_url: &Url, param_name: &str) -> Option<String> {
    parsed_url
        .query_pairs()
        .find(|(key, _)| key == param_name)
        .map(|(_, value)| value.into_owned())
}

fn read_authorization_code(parsed_url: &Url) -> Option<AuthorizationCode> {
    parsed_url
        .query_pairs()
        .find(|(key, _)| key == "code")
        .map(|(_, value)| AuthorizationCode::new(value.into_owned()))
}
