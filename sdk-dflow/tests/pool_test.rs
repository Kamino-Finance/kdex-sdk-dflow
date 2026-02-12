//! Integration test: Testing localhost pools using DFlow interface
//!
//! This test automatically discovers and tests all KDEX pools on localhost.
//! Make sure your local validator is running with the pools deployed.
//!
//! Each pool is tested with two vault capacity configurations:
//! - Default (98%): Preemptive estimation with 98% target
//! - Binary search (0): No preemptive cap, binary search on overflow
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

/// Vault capacity configurations to test
const CAPACITY_CONFIGS: &[(u16, &str)] = &[(9800, "preemptive 98%"), (0, "binary search")];

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

/// Run quote tests on a single AMM instance.
///
/// Tests small quotes, reverse direction, near-limit quotes, excessive amounts,
/// and the update_if_changed() optimization.
fn test_pool_quotes(
    amm: &mut KDEXAmm,
    mints: &[Pubkey],
    is_oracle: bool,
    accounts_map: &AccountMap,
    oracle_accounts_map: &AccountMap,
) {
    // Test quotes (A -> B)
    println!("\n  Testing quotes (Token A -> Token B):");
    let test_amounts = if is_oracle {
        vec![1_000_000u64, 5_000_000, 10_000_000]
    } else {
        vec![100u64, 250, 500]
    };

    for amount in test_amounts {
        let result = if is_oracle {
            amm.quote_oracle(
                &QuoteParams {
                    input_mint: mints[0],
                    output_mint: mints[1],
                    amount,
                    swap_mode: SwapMode::ExactIn,
                },
                oracle_accounts_map,
            )
        } else {
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

    // Test reverse direction (B -> A)
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
            oracle_accounts_map,
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

    // Test quotes near liquidity limits (A->B direction)
    println!("\n  Testing quotes near liquidity limits (A->B direction):");
    let near_limit_inputs = if is_oracle {
        vec![
            (455_000_000u64, "~85% vault"),
            (482_000_000u64, "~90% vault"),
            (509_000_000u64, "~95% vault"),
            (525_000_000u64, "~98% vault"),
            (532_500_000u64, "~99.9% vault"),
        ]
    } else {
        vec![]
    };

    for (amount, label) in near_limit_inputs {
        let result = if is_oracle {
            amm.quote_oracle(
                &QuoteParams {
                    input_mint: mints[0],
                    output_mint: mints[1],
                    amount,
                    swap_mode: SwapMode::ExactIn,
                },
                oracle_accounts_map,
            )
        } else {
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
                    "    {} {}: {} in -> {} out (requested {})",
                    label,
                    if quote.in_amount < amount {
                        "[CAPPED]"
                    } else {
                        ""
                    },
                    quote.in_amount,
                    quote.out_amount,
                    amount
                );
            }
            Err(e) => {
                println!("    {} Quote failed: {}", label, e);
            }
        }
    }

    // Test reverse direction near liquidity limits (B->A direction)
    println!("\n  Testing quotes near liquidity limits (B->A direction):");
    let reverse_near_limit = if is_oracle {
        vec![(50u64, "~99.9% vault")]
    } else {
        vec![]
    };

    for (amount, label) in reverse_near_limit {
        let result = if is_oracle {
            amm.quote_oracle(
                &QuoteParams {
                    input_mint: mints[1],
                    output_mint: mints[0],
                    amount,
                    swap_mode: SwapMode::ExactIn,
                },
                oracle_accounts_map,
            )
        } else {
            amm.quote(&QuoteParams {
                input_mint: mints[1],
                output_mint: mints[0],
                amount,
                swap_mode: SwapMode::ExactIn,
            })
        };

        match result {
            Ok(quote) => {
                println!(
                    "    {} {}: {} in -> {} out (requested {})",
                    label,
                    if quote.in_amount < amount {
                        "[CAPPED]"
                    } else {
                        ""
                    },
                    quote.in_amount,
                    quote.out_amount,
                    amount
                );
            }
            Err(e) => {
                println!("    {} Quote failed: {}", label, e);
            }
        }
    }

    // Test quote with amount exceeding liquidity (A->B direction)
    println!("\n  Testing quote with excessive amount (A->B liquidity check):");
    let excessive_amount = u64::MAX / 2;
    let result = if is_oracle {
        amm.quote_oracle(
            &QuoteParams {
                input_mint: mints[0],
                output_mint: mints[1],
                amount: excessive_amount,
                swap_mode: SwapMode::ExactIn,
            },
            oracle_accounts_map,
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
            println!(
                "    Quote returned: {} in -> {} out (requested {})",
                quote.in_amount, quote.out_amount, excessive_amount
            );
        }
        Err(e) => {
            if format!("{}", e).contains("liquidity") || format!("{}", e).contains("Insufficient") {
                println!("    Quote correctly rejected excessive amount: {}", e);
            } else {
                println!("    Quote failed (may be expected): {}", e);
            }
        }
    }

    // Test quote with excessive amount (B->A direction)
    if is_oracle {
        println!("\n  Testing quote with overcapacity (B->A liquidity check):");
        let excessive_amount_reverse = 50_000u64;
        let result = amm.quote_oracle(
            &QuoteParams {
                input_mint: mints[1],
                output_mint: mints[0],
                amount: excessive_amount_reverse,
                swap_mode: SwapMode::ExactIn,
            },
            oracle_accounts_map,
        );

        match result {
            Ok(quote) => {
                println!(
                    "    {} Quote returned: {} in -> {} out (requested {})",
                    if quote.in_amount < excessive_amount_reverse {
                        "[CAPPED]"
                    } else {
                        ""
                    },
                    quote.in_amount,
                    quote.out_amount,
                    excessive_amount_reverse
                );
            }
            Err(e) => {
                if format!("{}", e).contains("liquidity")
                    || format!("{}", e).contains("Insufficient")
                {
                    println!("    Quote correctly rejected excessive amount: {}", e);
                } else {
                    println!("    Quote failed (may be expected): {}", e);
                }
            }
        }
    }

    // Test performance optimization
    println!("\n  Testing update_if_changed() performance:");
    match amm.update_if_changed(accounts_map) {
        Ok(changed) => {
            println!("    Accounts changed: {}", changed);
            println!("    (Should be false on second call with same data)");
        }
        Err(e) => {
            println!("    update_if_changed failed: {}", e);
        }
    }
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

    // Test each pool with each capacity configuration
    for (i, (pool_address, account)) in pools.into_iter().enumerate() {
        let pool_num = i.saturating_add(1);
        println!("========== Pool {} ==========", pool_num);
        println!("Pool address: {}", pool_address);

        for &(target_bps, config_label) in CAPACITY_CONFIGS {
            println!(
                "\n--- Pool {} / vault_capacity_target_bps={} ({}) ---",
                pool_num, target_bps, config_label
            );

            // Create AMM instance with custom program ID and capacity config
            let mut amm = match KDEXAmm::new_from_keyed_account_with_program_id(
                &KeyedAccount {
                    key: pool_address,
                    account: account.clone(),
                    params: None,
                },
                program_id,
            ) {
                Ok(amm) => amm,
                Err(e) => {
                    println!("Failed to create AMM: {}", e);
                    println!("   The account might not be a valid KDEX pool\n");
                    break; // No point trying the other config
                }
            };

            // Apply capacity config
            amm = match amm.with_vault_capacity_target(target_bps) {
                Ok(amm) => amm,
                Err(e) => {
                    println!(
                        "Failed to set vault_capacity_target_bps={}: {}",
                        target_bps, e
                    );
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
                    break;
                }
            };

            let accounts_map: AccountMap = accounts_to_update
                .iter()
                .zip(accounts.iter())
                .filter_map(|(key, acc)| acc.as_ref().map(|a| (*key, a.clone())))
                .collect();

            if let Err(e) = amm.update(&accounts_map) {
                println!("Failed to update AMM: {}", e);
                break;
            }

            println!("  Vaults updated");

            // Check if this is an oracle curve and fetch Scope price feed if needed
            let is_oracle = matches!(
                amm.curve_type(),
                CurveType::ConstantSpreadOracle | CurveType::InventorySkewOracle
            );

            let mut oracle_accounts_map = accounts_map.clone();

            if is_oracle {
                println!("  Oracle curve detected - fetching Scope price feed");

                let scope_price_feed = match amm.get_scope_price_feed(&accounts_map) {
                    Some(feed) => feed,
                    None => {
                        println!("  Failed to get Scope price feed from curve\n");
                        break;
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
                        break;
                    }
                }
            }

            test_pool_quotes(
                &mut amm,
                &mints,
                is_oracle,
                &accounts_map,
                &oracle_accounts_map,
            );

            println!("\n  Config {} test complete", config_label);
        }

        println!("\n========== Pool {} complete ==========\n", pool_num);
    }

    println!("=== All tests complete ===");
    Ok(())
}
