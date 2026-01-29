use acp_jcp::{
    auth::{authenticate_and_get_refresh_token, get_access_token},
    keychain::get_refresh_token,
};

pub fn main() {
    let token = if let Some(refresh_token) = get_refresh_token().unwrap() {
        get_access_token(&refresh_token).unwrap()
    } else {
        authenticate_and_get_refresh_token().unwrap()
    };
    eprintln!("=== Authentication Successful ===\n");
    eprintln!("Access Token:\n  {}\n", token);
}
