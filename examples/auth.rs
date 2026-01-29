use acp_jcp::{
    auth::{authenticate, get_access_token},
    keychain::get_refresh_token,
};

pub fn main() {
    // Try to get refresh token from keychain and upgrade it
    let token = if let Some(refresh_token) = get_refresh_token().unwrap() {
        get_access_token(&refresh_token).unwrap()
    } else {
        authenticate().unwrap()
    };
    println!("=== Authentication Successful ===\n");
    println!("Access Token:\n  {}\n", token);
}
