use jcp::{
    auth::{get_access_token, login},
    keychain,
};

#[tokio::main]
async fn main() {
    let keychain = keychain::platform_keychain();
    let token = if let Some(refresh_token) = keychain.get_refresh_token().unwrap() {
        get_access_token(&refresh_token).await.unwrap()
    } else {
        login().await.unwrap()
    };
    eprintln!("=== Authentication Successful ===\n");
    eprintln!("Access Token:\n  {}\n", token);
}
