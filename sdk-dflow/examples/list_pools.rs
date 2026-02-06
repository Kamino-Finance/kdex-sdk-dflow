//! Example: List all KDEX pools on localhost
//!
//! This example scans your localhost for KDEX pool accounts.
//!
//! Run with: cargo run --example list_pools --features testing
//!
//! Environment variables:
//! - KDEX_PROGRAM_ID: Program ID (default: kdexv89r17wFQN1MY3auCX7QgWFyshWAji2LsLRVUQU)
//! - RPC: RPC endpoint URL (default: http://127.0.0.1:8899)

use solana_client::rpc_client::RpcClient;
use solana_sdk::hash::hash;
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;

/// Default program ID (production)
const DEFAULT_PROGRAM_ID: &str = "kdexv89r17wFQN1MY3auCX7QgWFyshWAji2LsLRVUQU";

/// Get the KDEX program ID from environment or use default
fn get_program_id() -> Pubkey {
    let program_str =
        std::env::var("KDEX_PROGRAM_ID").unwrap_or_else(|_| DEFAULT_PROGRAM_ID.to_string());
    Pubkey::from_str(&program_str).expect("Invalid KDEX_PROGRAM_ID")
}

/// Returns the 8-byte discriminator for SwapPool accounts
/// Computed as: hash("account:SwapPool")[..8]
fn swap_pool_discriminator() -> [u8; 8] {
    let discriminator_preimage = format!("account:{}", "SwapPool");
    let hash_result = hash(discriminator_preimage.as_bytes());
    let mut discriminator = [0u8; 8];
    discriminator.copy_from_slice(&hash_result.to_bytes()[..8]);
    discriminator
}

fn main() -> anyhow::Result<()> {
    println!("=== KDEX Pool Discovery (DFlow SDK) ===\n");

    let rpc_url = std::env::var("RPC").unwrap_or_else(|_| "http://127.0.0.1:8899".to_string());
    let rpc = RpcClient::new(rpc_url);
    let program_id = get_program_id();

    println!("Connecting to localhost...");
    println!("Program ID: {}\n", program_id);

    // Try to get program accounts
    println!("Scanning for KDEX pools...");

    match rpc.get_program_accounts(&program_id) {
        Ok(accounts) => {
            println!(
                "Found {} account(s) owned by KDEX program\n",
                accounts.len()
            );

            // Filter for SwapPool accounts by checking discriminator
            let discriminator = swap_pool_discriminator();
            let pools: Vec<_> = accounts
                .iter()
                .filter(|(_, account)| {
                    account.data.len() >= 8 && account.data[0..8] == discriminator
                })
                .collect();

            if pools.is_empty() {
                println!("No SwapPool accounts found. Make sure:");
                println!("  1. Your local validator is running");
                println!("  2. KDEX pools are deployed");
                println!("  3. You've created pools using the CLI");

                if !accounts.is_empty() {
                    println!(
                        "\nNote: Found {} non-pool accounts owned by the program",
                        accounts.len()
                    );
                    println!("(These could be curve accounts, vaults, etc.)\n");
                } else {
                    println!();
                }
            } else {
                println!("Found {} SwapPool account(s):\n", pools.len());
                for (i, (pubkey, account)) in pools.iter().enumerate() {
                    println!("Pool {}: {}", i.saturating_add(1), pubkey);
                    println!("  Data size: {} bytes", account.data.len());
                    println!("  Owner: {}", account.owner);
                    println!("  Lamports: {}\n", account.lamports);
                }
            }
        }
        Err(e) => {
            println!("Failed to get program accounts: {}", e);
            println!("\nMake sure:");
            println!("  1. Local validator is running: solana-test-validator");
            println!("  2. RPC is accessible at http://127.0.0.1:8899");
            println!("  3. Program is deployed: anchor build && anchor deploy\n");
        }
    }

    Ok(())
}
