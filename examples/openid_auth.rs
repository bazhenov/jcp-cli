//! OpenID Connect / OAuth2 Authorization Code Flow Example
//!
//! This example demonstrates:
//! 1. Starting a local server to receive the OAuth callback
//! 2. Opening the user's browser to authenticate with an OpenID provider
//! 3. Exchanging the authorization code for tokens
//! 4. Printing the credentials to the CLI
//!
//! Usage:
//!   Set environment variables:
//!     - OIDC_CLIENT_ID: Your OAuth2 client ID
//!     - OIDC_CLIENT_SECRET: Your OAuth2 client secret (optional for public clients)
//!     - OIDC_AUTH_URL: Authorization endpoint URL
//!     - OIDC_TOKEN_URL: Token endpoint URL
//!
//!   Then run:
//!     cargo run --example openid_auth

use oauth2::{
    AuthUrl, AuthorizationCode, ClientId, ClientSecret, CsrfToken, PkceCodeChallenge, RedirectUrl,
    Scope, TokenResponse, TokenUrl, basic::BasicClient,
};
use std::{
    io::{BufRead, BufReader, Write},
    net::TcpListener,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Load configuration from environment
    let base_url = "https://public.aip.oauth.intservices.aws.intellij.net";
    let client_id = "air".to_string();
    let client_secret = None;
    let auth_url = format!("{}/oauth2/auth", base_url);
    let token_url = format!("{}/oauth2/token", base_url);

    let listener = TcpListener::bind(format!("127.0.0.1:0"))?;

    let local_port = listener.local_addr()?.port();

    let redirect_url = format!("http://localhost:{}/space/auth", local_port);

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
        //.add_scope(Scope::new("offline_access".to_string())) // for retrieveing refresh_token
        .add_scope(Scope::new("openid".to_string()))
        .set_pkce_challenge(pkce_challenge)
        .url();

    println!("Opening browser for authentication...");
    println!("If the browser doesn't open, visit this URL manually:");
    println!("\n  {}\n", auth_url);

    // Try to open the browser
    if let Err(e) = open::that(auth_url.to_string()) {
        eprintln!("Failed to open browser: {}", e);
    }

    // Start local server to receive the callback
    println!(
        "Waiting for authentication callback on port {}...",
        local_port
    );

    // Accept the callback connection
    let (mut stream, _) = listener.accept()?;

    // Read the HTTP request
    let mut reader = BufReader::new(&stream);
    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;

    // Parse the callback URL to extract code and state
    let redirect_url = request_line
        .split_whitespace()
        .nth(1)
        .ok_or("Invalid HTTP request")?;

    let url = url::Url::parse(&format!("http://localhost{}", redirect_url))?;

    let code = url
        .query_pairs()
        .find(|(key, _)| key == "code")
        .map(|(_, value)| AuthorizationCode::new(value.into_owned()))
        .ok_or("Authorization code not found in callback")?;

    let state = url
        .query_pairs()
        .find(|(key, _)| key == "state")
        .map(|(_, value)| CsrfToken::new(value.into_owned()))
        .ok_or("State not found in callback")?;

    // Verify CSRF token
    if state.secret() != csrf_token.secret() {
        return Err("CSRF token mismatch! Possible attack.".into());
    }

    // Send success response to browser
    let response = "HTTP/1.1 200 OK\r\n\
                    Content-Type: text/html\r\n\
                    Connection: close\r\n\r\n\
                    <html><body>\
                    <h1>Authentication Successful!</h1>\
                    <p>You can close this window and return to the terminal.</p>\
                    </body></html>";
    stream.write_all(response.as_bytes())?;
    drop(stream);

    println!("\nReceived authorization code, exchanging for tokens...\n");

    // Exchange the authorization code for tokens
    // Disable redirects to prevent SSRF vulnerabilities
    let http_client = reqwest::blocking::ClientBuilder::new()
        .redirect(reqwest::redirect::Policy::none())
        .build()?;

    let token_result = client
        .exchange_code(code)
        .set_pkce_verifier(pkce_verifier)
        .request(&http_client)?;

    // Print the tokens
    println!("=== Authentication Successful ===\n");
    println!(
        "Access Token:\n  {}\n",
        token_result.access_token().secret()
    );

    if let Some(refresh_token) = token_result.refresh_token() {
        println!("Refresh Token:\n  {}\n", refresh_token.secret());
    }

    if let Some(expires_in) = token_result.expires_in() {
        println!("Expires In: {:?}\n", expires_in);
    }

    if let Some(scopes) = token_result.scopes() {
        let scope_strs: Vec<_> = scopes.iter().map(|s| s.as_str()).collect();
        println!("Scopes: {}\n", scope_strs.join(", "));
    }

    Ok(())
}
