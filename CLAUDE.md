# Running ant testing project

- do not run project directly using `cargo run` it will block stdin
- use `cargo check` and `cargo test` to test that projects works

# Terms

- downlink - communcation channel between ACP client (IDE) and this program (adapter)
- uplink - communication channel between this program (adapter) and ACP server (JCP)

# Rules

- when having a struct for representing JSON payload using serde, use `#[serde(rename = ...)]` to specify field name in JSON and prevent any possible problems when renaming Rust field names