//! Integration test: Testing localhost pools using DFlow interface
//!
//! This test automatically discovers and tests all KDEX pools on localhost.
//! Make sure your local validator is running with the pools deployed.
//!
//! Run with: cargo test --test pool_test --features testing -- --ignored --nocapture
//!
//! Environment variables:
//! - KDEX_PROGRAM_ID: Program ID (default: kdexv89r17wFQN1MY3auCX7QgWFyshWAji2LsLRVUQU)
//! - RPC: RPC endpoint URL (default: http://127.0.0.1:8899)

use kdex_client::CurveType;
use kdex_sdk_dflow::{AccountMap, Amm, KDEXAmm, KeyedAccount, QuoteParams, SwapMode};
use solana_client::rpc_client::RpcClient;
use solana_sdk::{account::Account, hash::hash, pubkey::Pubkey};
use std::str::FromStr;

/// Default program ID (staging)
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

/// Discovers all SwapPool accounts owned by the KDEX program
fn discover_pools(rpc: &RpcClient, program_id: &Pubkey) -> anyhow::Result<Vec<(Pubkey, Account)>> {
    let accounts = rpc.get_program_accounts(program_id)?;
    let discriminator = swap_pool_discriminator();

    let pools: Vec<_> = accounts
        .into_iter()
        .filter(|(_, account)| account.data.len() >= 8 && account.data[0..8] == discriminator)
        .collect();

    Ok(pools)
}

#[test]
#[ignore] // Run with: cargo test --test pool_test --features testing -- --ignored --nocapture
fn test_all_localhost_pools() {
    if let Err(e) = run_pool_tests() {
        panic!("Pool test failed: {}", e);
    }
}

fn run_pool_tests() -> anyhow::Result<()> {
    println!("=== KDEX SDK (DFlow) - Localhost Pool Testing ===\n");

    // Connect to localhost
    let rpc_url = std::env::var("RPC").unwrap_or_else(|_| "http://127.0.0.1:8899".to_string());
    let rpc = RpcClient::new(rpc_url);

    // Parse program ID from environment or use default
    let program_id = get_program_id();
    println!("Program ID: {}", program_id);

    // Discover pools
    println!("Scanning for KDEX pools...\n");
    let pools = match discover_pools(&rpc, &program_id) {
        Ok(pools) => pools,
        Err(e) => {
            println!("Failed to discover pools: {}", e);
            println!("\nMake sure:");
            println!("  1. Local validator is running: solana-test-validator");
            println!("  2. RPC is accessible at {}", rpc.url());
            println!("  3. Pools are deployed: Use the CLI to create pools\n");
            return Ok(());
        }
    };

    if pools.is_empty() {
        println!("No pools found. Make sure:");
        println!("  1. Your local validator is running");
        println!("  2. KDEX pools are deployed");
        println!("  3. You've created pools using the CLI\n");
        return Ok(());
    }

    println!("Found {} pool(s)\n", pools.len());

    // Test each pool
    for (i, (pool_address, account)) in pools.into_iter().enumerate() {
        println!("--- Testing Pool {} ---", i.saturating_add(1));
        println!("Pool address: {}", pool_address);

        // Create AMM instance with custom program ID
        let mut amm = match KDEXAmm::new_from_keyed_account_with_program_id(
            &KeyedAccount {
                key: pool_address,
                account,
                params: None,
            },
            program_id,
        ) {
            Ok(amm) => amm,
            Err(e) => {
                println!("Failed to create AMM: {}", e);
                println!("   The account might not be a valid KDEX pool\n");
                continue;
            }
        };

        // Display pool info
        println!("Pool loaded successfully");
        println!("  Curve type: {:?}", amm.curve_type());
        println!("  Withdrawals only: {}", amm.is_withdrawals_only());

        let mints = amm.get_reserve_mints();
        println!("  Token A: {}", mints[0]);
        println!("  Token B: {}", mints[1]);

        // Update vault balances
        let accounts_to_update = amm.get_accounts_to_update();
        println!("  Accounts to update: {:?}", accounts_to_update.len());

        let accounts = match rpc.get_multiple_accounts(&accounts_to_update) {
            Ok(accs) => accs,
            Err(e) => {
                println!("Failed to fetch vault accounts: {}", e);
                println!();
                continue;
            }
        };

        let accounts_map: AccountMap = accounts_to_update
            .iter()
            .zip(accounts.iter())
            .filter_map(|(key, acc)| acc.as_ref().map(|a| (*key, a.clone())))
            .collect();

        if let Err(e) = amm.update(&accounts_map) {
            println!("Failed to update AMM: {}", e);
            println!();
            continue;
        }

        println!("  Vaults updated");

        // Check if this is an oracle curve and fetch Scope price feed if needed
        let is_oracle = matches!(
            amm.curve_type(),
            CurveType::ConstantSpreadOracle | CurveType::InventorySkewOracle
        );

        // Extended accounts map for oracle pools
        let mut oracle_accounts_map = accounts_map.clone();

        if is_oracle {
            println!("  Oracle curve detected - fetching Scope price feed");

            // Get Scope price feed address dynamically from the curve
            let scope_price_feed = match amm.get_scope_price_feed(&accounts_map) {
                Some(feed) => feed,
                None => {
                    println!("  Failed to get Scope price feed from curve\n");
                    continue;
                }
            };
            println!("  Scope price feed: {}", scope_price_feed);

            match rpc.get_account(&scope_price_feed) {
                Ok(scope_account) => {
                    oracle_accounts_map.insert(scope_price_feed, scope_account);
                    println!("  Scope price feed fetched");
                }
                Err(e) => {
                    println!("  Failed to fetch Scope price feed: {}", e);
                    println!("     Oracle quotes will not work\n");
                    continue;
                }
            }
        }

        // Test quotes
        println!("\n  Testing quotes (Token A -> Token B):");
        // Oracle pools need larger A→B amounts (price ~10^-6 B/A means ~10^6 raw A per raw B)
        let test_amounts = if is_oracle {
            vec![1_000_000u64, 5_000_000, 10_000_000]
        } else {
            vec![100u64, 250, 500]
        };

        for amount in test_amounts {
            let result = if is_oracle {
                // Use oracle quote method
                amm.quote_oracle(
                    &QuoteParams {
                        input_mint: mints[0],
                        output_mint: mints[1],
                        amount,
                        swap_mode: SwapMode::ExactIn,
                    },
                    &oracle_accounts_map,
                )
            } else {
                // Use standard quote method
                amm.quote(&QuoteParams {
                    input_mint: mints[0],
                    output_mint: mints[1],
                    amount,
                    swap_mode: SwapMode::ExactIn,
                })
            };

            match result {
                Ok(quote) => {
                    println!(
                        "    {} in -> {} out (requested {})",
                        quote.in_amount, quote.out_amount, amount
                    );
                }
                Err(e) => {
                    println!("    Quote failed for amount {}: {}", amount, e);
                }
            }
        }

        // Test reverse direction
        // Oracle pools: each raw B worth ~10^6 raw A, so use small B amount
        let reverse_amount = if is_oracle { 10u64 } else { 10_000_000 };
        println!("\n  Testing quotes (Token B -> Token A):");
        let result = if is_oracle {
            amm.quote_oracle(
                &QuoteParams {
                    input_mint: mints[1],
                    output_mint: mints[0],
                    amount: reverse_amount,
                    swap_mode: SwapMode::ExactIn,
                },
                &oracle_accounts_map,
            )
        } else {
            amm.quote(&QuoteParams {
                input_mint: mints[1],
                output_mint: mints[0],
                amount: reverse_amount,
                swap_mode: SwapMode::ExactIn,
            })
        };

        match result {
            Ok(quote) => {
                println!(
                    "    {} in -> {} out (requested {})",
                    quote.in_amount, quote.out_amount, reverse_amount
                );
            }
            Err(e) => {
                println!("    Quote failed: {}", e);
            }
        }

        // Test quote with amount exceeding liquidity
        println!("\n  Testing quote with excessive amount (liquidity check):");
        let excessive_amount = u64::MAX / 2; // Very large amount
        let result = if is_oracle {
            amm.quote_oracle(
                &QuoteParams {
                    input_mint: mints[0],
                    output_mint: mints[1],
                    amount: excessive_amount,
                    swap_mode: SwapMode::ExactIn,
                },
                &oracle_accounts_map,
            )
        } else {
            amm.quote(&QuoteParams {
                input_mint: mints[0],
                output_mint: mints[1],
                amount: excessive_amount,
                swap_mode: SwapMode::ExactIn,
            })
        };

        match result {
            Ok(quote) => {
                // For AMM curves, the math naturally limits output
                // For oracle curves, should return InsufficientLiquidity error
                println!(
                    "    Quote returned: {} in -> {} out (requested {})",
                    quote.in_amount, quote.out_amount, excessive_amount
                );
            }
            Err(e) => {
                // Expected for oracle curves when exceeding liquidity
                if format!("{}", e).contains("liquidity")
                    || format!("{}", e).contains("Insufficient")
                {
                    println!("    Quote correctly rejected excessive amount: {}", e);
                } else {
                    println!("    Quote failed (may be expected): {}", e);
                }
            }
        }

        // Test performance optimization
        println!("\n  Testing update_if_changed() performance:");
        match amm.update_if_changed(&accounts_map) {
            Ok(changed) => {
                println!("    Accounts changed: {}", changed);
                println!("    (Should be false on second call with same data)");
            }
            Err(e) => {
                println!("    update_if_changed failed: {}", e);
            }
        }

        println!("\nPool {} test complete\n", i.saturating_add(1));
    }

    println!("=== All tests complete ===");
    Ok(())
}
