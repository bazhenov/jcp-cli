use oauth2::{
    AuthUrl, AuthorizationCode, ClientId, ClientSecret, CsrfToken, PkceCodeChallenge, RedirectUrl,
    Scope, TokenResponse, TokenUrl, basic::BasicClient,
};
use reqwest::redirect::Policy;
use std::error::Error;
use tiny_http::{Response, Server};
use url::Url;

/// The expected callback path for OAuth redirect
const CALLBACK_PATH: &str = "/space/auth";

pub fn authenticate_get_token() -> Result<String, Box<dyn Error + Send + Sync>> {
    // Load configuration
    let base_url = "https://public.aip.oauth.intservices.aws.intellij.net";
    let client_id = "air".to_string();
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
    let mut client = BasicClient::new(ClientId::new(client_id))
        .set_auth_uri(AuthUrl::new(auth_url)?)
        .set_token_uri(TokenUrl::new(token_url)?)
        .set_redirect_uri(RedirectUrl::new(redirect_url)?);

    if let Some(secret) = client_secret {
        client = client.set_client_secret(ClientSecret::new(secret));
    }

    // Generate PKCE challenge (recommended for all clients, required for public clients)
    let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();

    // Generate the authorization URL
    let (auth_url, csrf_token) = client
        .authorize_url(CsrfToken::new_random)
        //.add_scope(Scope::new("offline_access".to_string())) // for retrieving refresh_token
        .add_scope(Scope::new("openid".to_string()))
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

    let token_result = client
        .exchange_code(code)
        .set_pkce_verifier(pkce_verifier)
        .request(&http_client)?;

    Ok(token_result.access_token().secret().clone())
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
