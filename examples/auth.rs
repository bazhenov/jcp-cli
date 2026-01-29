use acp_jcp::auth::authenticate;

pub fn main() {
    let token = authenticate().unwrap();
    println!("=== Authentication Successful ===\n");
    println!("Access Token:\n  {}\n", token);
}
