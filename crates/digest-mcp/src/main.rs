//! Standalone `stdio` MCP binary entrypoint (implemented fully in Round 4).

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use digest_core::constants::SERVICE_NAME;

#[tokio::main]
async fn main() {
    println!("{SERVICE_NAME} MCP server scaffold ready (full stdio/HTTP transport in Round 4).");
}
