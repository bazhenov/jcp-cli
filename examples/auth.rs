use acp_jcp::auth::authenticate_get_token;

pub fn main() {
    let code = authenticate_get_token().unwrap();
    println!("=== Authentication Successful ===\n");
    println!("Access Token:\n  {}\n", code);
}
