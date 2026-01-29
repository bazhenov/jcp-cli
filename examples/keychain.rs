//! Minimal example of storing and reading a string from macOS Keychain
//!
//! Usage:
//!   cargo run --example keychain

use security_framework::passwords::{
    PasswordOptions, delete_generic_password, get_generic_password, set_generic_password_options,
};

const SERVICE: &str = "com.jetbrains.acp-jcp";
const ACCOUNT: &str = "test-account";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let secret = "my-secret-value-12345";

    // Store a value
    println!("Storing secret in Keychain...");
    let opts = PasswordOptions::new_generic_password(SERVICE, ACCOUNT);
    //    opts.set_access_control_options(AccessControlOptions::DEVICE_PASSCODE);
    set_generic_password_options(secret.as_bytes(), opts)?;
    println!("  Stored: {}", secret);

    // Read it back
    println!("\nReading secret from Keychain...");
    let retrieved = get_generic_password(SERVICE, ACCOUNT)?;
    let retrieved_str = String::from_utf8(retrieved)?;
    println!("  Retrieved: {}", retrieved_str);

    // Verify
    assert_eq!(secret, retrieved_str);
    println!("\nSuccess! Values match.");

    // Clean up
    println!("\nDeleting secret from Keychain...");
    delete_generic_password(SERVICE, ACCOUNT)?;
    println!("  Deleted.");

    // Verify deletion
    match get_generic_password(SERVICE, ACCOUNT) {
        Ok(_) => println!("  Warning: Secret still exists!"),
        Err(_) => println!("  Confirmed: Secret no longer exists."),
    }

    Ok(())
}
