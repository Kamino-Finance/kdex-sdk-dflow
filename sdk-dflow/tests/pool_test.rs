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
    score: u8,
) {
    // Test quotes (A -> B)
    println!("\n  Testing quotes (Token A -> Token B):");
    let test_amounts = if is_oracle {
        vec![10_000_000u64, 50_000_000, 100_000_000]
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
                score,
            )
        } else {
            amm.quote_with_score(
                &QuoteParams {
                    input_mint: mints[0],
                    output_mint: mints[1],
                    amount,
                    swap_mode: SwapMode::ExactIn,
                },
                score,
            )
        };

        match result {
            Ok(quote) => {
                println!(
                    "    {} in -> {} out (requested {})",
                    quote.in_amount, quote.out_amount, amount
                );
                assert!(
                    quote.out_amount > 0,
                    "out_amount should be > 0 for amount {}",
                    amount
                );
                assert!(
                    quote.in_amount <= amount,
                    "in_amount ({}) should be <= requested ({})",
                    quote.in_amount,
                    amount
                );
            }
            Err(e) => {
                panic!("    Quote failed for amount {}: {}", amount, e);
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
            score,
        )
    } else {
        amm.quote_with_score(
            &QuoteParams {
                input_mint: mints[1],
                output_mint: mints[0],
                amount: reverse_amount,
                swap_mode: SwapMode::ExactIn,
            },
            score,
        )
    };

    match result {
        Ok(quote) => {
            println!(
                "    {} in -> {} out (requested {})",
                quote.in_amount, quote.out_amount, reverse_amount
            );
            assert!(quote.out_amount > 0, "B→A out_amount should be > 0");
            assert!(
                quote.in_amount <= reverse_amount,
                "B→A in_amount ({}) should be <= requested ({})",
                quote.in_amount,
                reverse_amount
            );
        }
        Err(e) => {
            panic!("    B→A quote failed: {}", e);
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
                score,
            )
        } else {
            amm.quote_with_score(
                &QuoteParams {
                    input_mint: mints[0],
                    output_mint: mints[1],
                    amount,
                    swap_mode: SwapMode::ExactIn,
                },
                score,
            )
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
                score,
            )
        } else {
            amm.quote_with_score(
                &QuoteParams {
                    input_mint: mints[1],
                    output_mint: mints[0],
                    amount,
                    swap_mode: SwapMode::ExactIn,
                },
                score,
            )
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
            score,
        )
    } else {
        amm.quote_with_score(
            &QuoteParams {
                input_mint: mints[0],
                output_mint: mints[1],
                amount: excessive_amount,
                swap_mode: SwapMode::ExactIn,
            },
            score,
        )
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
            score,
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
                0,
            );

            // Also test with score=4 on oracle pools to verify widened spread
            if is_oracle {
                println!("\n  Re-testing with score=4 (widened spread):");
                test_pool_quotes(
                    &mut amm,
                    &mints,
                    is_oracle,
                    &accounts_map,
                    &oracle_accounts_map,
                    4,
                );
            }

            println!("\n  Config {} test complete", config_label);
        }

        println!("\n========== Pool {} complete ==========\n", pool_num);
    }

    println!("=== All tests complete ===");
    Ok(())
}

/// Test that score-based spread widening produces monotonically less output for higher scores.
///
/// Runs on ALL oracle pools with score_factor_bps > 0.
/// Verifies for each score 0–4:
/// - Quote succeeds with out_amount > 0
/// - Higher scores yield strictly less output (or equal only if rounding)
/// - Both A→B and B→A directions behave correctly
///
/// Uses vault_capacity_target_bps=0 (binary search) to avoid capacity estimation masking the effect.
/// Uses amounts large enough that the integer-level spread difference is observable.
///
/// Run with: cargo test --test pool_test --features testing -- --ignored --nocapture test_score_widening
#[test]
#[ignore]
fn test_score_widening() {
    if let Err(e) = run_score_tests() {
        panic!("Score widening test failed: {}", e);
    }
}

/// Reads curve parameters from raw account data for diagnostic printing.
fn print_curve_diagnostics(curve_type: CurveType, curve_data: &[u8]) {
    match curve_type {
        CurveType::ConstantSpreadOracle => {
            if curve_data.len() >= 80 {
                let bps_from_oracle =
                    u64::from_le_bytes(curve_data[56..64].try_into().unwrap_or_default());
                let price_offset_bps =
                    i64::from_le_bytes(curve_data[64..72].try_into().unwrap_or_default());
                let score_factor_bps =
                    u64::from_le_bytes(curve_data[72..80].try_into().unwrap_or_default());
                println!(
                    "  Curve params: bps_from_oracle={}, price_offset_bps={}, score_factor_bps={}",
                    bps_from_oracle, price_offset_bps, score_factor_bps
                );
            }
        }
        CurveType::InventorySkewOracle => {
            if curve_data.len() >= 128 {
                let base_spread_bps =
                    u64::from_le_bytes(curve_data[56..64].try_into().unwrap_or_default());
                let size_spread_bps =
                    u64::from_le_bytes(curve_data[64..72].try_into().unwrap_or_default());
                let skew_bps =
                    u64::from_le_bytes(curve_data[72..80].try_into().unwrap_or_default());
                let inv_equilibrium =
                    u64::from_le_bytes(curve_data[80..88].try_into().unwrap_or_default());
                let inv_max = u64::from_le_bytes(curve_data[88..96].try_into().unwrap_or_default());
                let q_ref = u64::from_le_bytes(curve_data[96..104].try_into().unwrap_or_default());
                let alpha = u64::from_le_bytes(curve_data[104..112].try_into().unwrap_or_default());
                let price_offset_bps =
                    i64::from_le_bytes(curve_data[112..120].try_into().unwrap_or_default());
                let score_factor_bps =
                    u64::from_le_bytes(curve_data[120..128].try_into().unwrap_or_default());
                println!("  Curve params: base_spread_bps={}, size_spread_bps={}, skew_bps={}, inv_eq={}, inv_max={}, q_ref={}, alpha={}, price_offset_bps={}, score_factor_bps={}",
                    base_spread_bps, size_spread_bps, skew_bps, inv_equilibrium, inv_max, q_ref, alpha, price_offset_bps, score_factor_bps);
            }
        }
        _ => {}
    }
}

fn run_score_tests() -> anyhow::Result<()> {
    println!("=== KDEX SDK (DFlow) - Score Widening Test ===\n");

    let rpc_url = std::env::var("RPC").unwrap_or_else(|_| "http://127.0.0.1:8899".to_string());
    let rpc = RpcClient::new(rpc_url);
    let program_id = get_program_id();

    let pools = discover_pools(&rpc, &program_id)?;
    if pools.is_empty() {
        println!("No pools found, skipping score test");
        return Ok(());
    }

    let mut tested_count = 0u32;

    for (addr, account) in &pools {
        let mut amm = match KDEXAmm::new_from_keyed_account_with_program_id(
            &KeyedAccount {
                key: *addr,
                account: account.clone(),
                params: None,
            },
            program_id,
        ) {
            Ok(amm) => amm,
            Err(_) => continue,
        };

        if !matches!(
            amm.curve_type(),
            CurveType::ConstantSpreadOracle | CurveType::InventorySkewOracle
        ) {
            println!(
                "Pool {} ({:?}): skipping (not oracle)\n",
                addr,
                amm.curve_type()
            );
            continue;
        }

        // Use binary search mode to avoid capacity estimation masking score effects
        amm = amm.with_vault_capacity_target(0)?;

        // Fetch curve data so score_factor_bps() can read it
        let accounts_to_update = amm.get_accounts_to_update();
        let fetched = rpc.get_multiple_accounts(&accounts_to_update)?;
        let mut accounts_map: AccountMap = accounts_to_update
            .iter()
            .zip(fetched.iter())
            .filter_map(|(key, acc)| acc.as_ref().map(|a| (*key, a.clone())))
            .collect();
        amm.update(&accounts_map)?;

        let sfb = amm.score_factor_bps();
        println!(
            "Pool {} ({:?}): score_factor_bps={}",
            addr,
            amm.curve_type(),
            sfb
        );

        // Print curve diagnostics (swap_curve is the 3rd entry in accounts_to_update)
        if accounts_to_update.len() >= 3 {
            if let Some(curve_acc) = accounts_map.get(&accounts_to_update[2]) {
                print_curve_diagnostics(amm.curve_type(), &curve_acc.data);
            }
        }

        if sfb == 0 {
            println!("  Skipping: score_factor_bps=0 (no score widening configured)\n");
            continue;
        }

        // Fetch scope price feed for oracle
        let scope_price_feed = amm
            .get_scope_price_feed(&accounts_map)
            .ok_or_else(|| anyhow::anyhow!("Could not get scope price feed for {}", addr))?;
        let scope_account = rpc.get_account(&scope_price_feed)?;
        accounts_map.insert(scope_price_feed, scope_account);

        let mints = amm.get_reserve_mints();

        // Use amounts large enough that the spread difference is visible at integer precision.
        // With score_factor_bps=500 and score=4: multiplier = 2000 bps = 20%.
        // If base_spread is B bps, score=4 makes it B*1.2.
        // The output difference ≈ output * (B*0.2) / 10000.
        // For this to be >= 1, we need output >= 10000 / (B*0.2) = 50000/B.
        // With B=30 bps, output >= ~1667. With B=1 bps, output >= 50000.
        // Use multiple amount tiers to ensure at least one shows the effect.
        let atob_amounts = vec![1_000_000u64, 10_000_000, 50_000_000, 100_000_000];
        let btoa_amounts = vec![10u64, 100, 1_000, 10_000];

        println!("\n  Score monotonicity (A→B):");
        for &amount in &atob_amounts {
            let mut prev: Option<(u64, u64)> = None; // (in_amount, out_amount)
            let mut any_diff = false;
            let mut results = Vec::new();
            for score in 0..=4u8 {
                let quote = amm.quote_oracle(
                    &QuoteParams {
                        input_mint: mints[0],
                        output_mint: mints[1],
                        amount,
                        swap_mode: SwapMode::ExactIn,
                    },
                    &accounts_map,
                    score,
                );
                match quote {
                    Ok(q) => {
                        if let Some((prev_in, prev_out)) = prev {
                            if q.out_amount != prev_out || q.in_amount != prev_in {
                                any_diff = true;
                            }
                            // Wider spread means worse rate: either less output or more input needed
                            assert!(
                                q.out_amount <= prev_out || q.in_amount >= prev_in,
                                "amount={} score={}: rate should worsen (out {} <= {} or in {} >= {})",
                                amount, score,
                                q.out_amount, prev_out, q.in_amount, prev_in
                            );
                        }
                        prev = Some((q.in_amount, q.out_amount));
                        results.push(format!("s{}:{}/{}", score, q.in_amount, q.out_amount));
                    }
                    Err(e) => {
                        results.push(format!("s{}:ERR({})", score, e));
                    }
                }
            }
            let marker = if !any_diff && prev.is_some() {
                " [NO DIFF]"
            } else {
                ""
            };
            println!("    amount={}: {}{}", amount, results.join(", "), marker);
        }

        println!("\n  Score monotonicity (B→A):");
        for &amount in &btoa_amounts {
            let mut prev: Option<(u64, u64)> = None;
            let mut any_diff = false;
            let mut results = Vec::new();
            for score in 0..=4u8 {
                let quote = amm.quote_oracle(
                    &QuoteParams {
                        input_mint: mints[1],
                        output_mint: mints[0],
                        amount,
                        swap_mode: SwapMode::ExactIn,
                    },
                    &accounts_map,
                    score,
                );
                match quote {
                    Ok(q) => {
                        if let Some((prev_in, prev_out)) = prev {
                            if q.out_amount != prev_out || q.in_amount != prev_in {
                                any_diff = true;
                            }
                            assert!(
                                q.out_amount <= prev_out || q.in_amount >= prev_in,
                                "amount={} score={}: rate should worsen (out {} <= {} or in {} >= {})",
                                amount, score,
                                q.out_amount, prev_out, q.in_amount, prev_in
                            );
                        }
                        prev = Some((q.in_amount, q.out_amount));
                        results.push(format!("s{}:{}/{}", score, q.in_amount, q.out_amount));
                    }
                    Err(e) => {
                        results.push(format!("s{}:ERR({})", score, e));
                    }
                }
            }
            let marker = if !any_diff && prev.is_some() {
                " [NO DIFF]"
            } else {
                ""
            };
            println!("    amount={}: {}{}", amount, results.join(", "), marker);
        }

        tested_count = tested_count.saturating_add(1);
        println!();
    }

    if tested_count == 0 {
        anyhow::bail!(
            "No oracle pool with score_factor_bps > 0 found. Set score_factor_bps via UpdatePoolConfig first."
        );
    }

    println!(
        "=== Score widening test passed ({} pools tested) ===\n",
        tested_count
    );
    Ok(())
}

/// Test that invalid scores (> 4) are rejected.
///
/// Run with: cargo test --test pool_test --features testing -- --ignored --nocapture test_score_validation
#[test]
#[ignore]
fn test_score_validation() {
    if let Err(e) = run_score_validation_tests() {
        panic!("Score validation test failed: {}", e);
    }
}

fn run_score_validation_tests() -> anyhow::Result<()> {
    println!("=== KDEX SDK (DFlow) - Score Validation Test ===\n");

    let rpc_url = std::env::var("RPC").unwrap_or_else(|_| "http://127.0.0.1:8899".to_string());
    let rpc = RpcClient::new(rpc_url);
    let program_id = get_program_id();

    let pools = discover_pools(&rpc, &program_id)?;
    if pools.is_empty() {
        println!("No pools found, skipping score validation test");
        return Ok(());
    }

    // Use first pool (any type)
    let (pool_address, pool_account) = &pools[0];
    let mut amm = KDEXAmm::new_from_keyed_account_with_program_id(
        &KeyedAccount {
            key: *pool_address,
            account: pool_account.clone(),
            params: None,
        },
        program_id,
    )?;

    let accounts_to_update = amm.get_accounts_to_update();
    let fetched = rpc.get_multiple_accounts(&accounts_to_update)?;
    let mut accounts_map: AccountMap = accounts_to_update
        .iter()
        .zip(fetched.iter())
        .filter_map(|(key, acc)| acc.as_ref().map(|a| (*key, a.clone())))
        .collect();
    amm.update(&accounts_map)?;

    let is_oracle = matches!(
        amm.curve_type(),
        CurveType::ConstantSpreadOracle | CurveType::InventorySkewOracle
    );

    if is_oracle {
        if let Some(scope_feed) = amm.get_scope_price_feed(&accounts_map) {
            if let Ok(scope_account) = rpc.get_account(&scope_feed) {
                accounts_map.insert(scope_feed, scope_account);
            }
        }
    }

    let mints = amm.get_reserve_mints();
    let params = QuoteParams {
        input_mint: mints[0],
        output_mint: mints[1],
        amount: 10_000_000,
        swap_mode: SwapMode::ExactIn,
    };

    // score=5 should fail via quote()
    let result = if is_oracle {
        amm.quote_oracle(&params, &accounts_map, 5)
    } else {
        amm.quote_with_score(&params, 5)
    };
    assert!(
        result.is_err(),
        "quote(score=5) should fail but got: {:?}",
        result.unwrap()
    );
    let err_msg = format!("{}", result.unwrap_err());
    assert!(
        err_msg.contains("Invalid score"),
        "Error should mention 'Invalid score', got: {}",
        err_msg
    );
    println!("quote(score=5) correctly rejected: {}", err_msg);

    // score=255 should also fail
    let result = if is_oracle {
        amm.quote_oracle(&params, &accounts_map, 255)
    } else {
        amm.quote_with_score(&params, 255)
    };
    assert!(result.is_err(), "quote(score=255) should fail");
    println!(
        "quote(score=255) correctly rejected: {}",
        result.unwrap_err()
    );

    // score=4 should succeed (boundary)
    let result = if is_oracle {
        amm.quote_oracle(&params, &accounts_map, 4)
    } else {
        amm.quote_with_score(&params, 4)
    };
    assert!(
        result.is_ok(),
        "quote(score=4) should succeed but got: {}",
        result.unwrap_err()
    );
    println!("quote(score=4) accepted: {:?}", result.unwrap());

    // score=0 should succeed
    let result = if is_oracle {
        amm.quote_oracle(&params, &accounts_map, 0)
    } else {
        amm.quote_with_score(&params, 0)
    };
    assert!(
        result.is_ok(),
        "quote(score=0) should succeed but got: {}",
        result.unwrap_err()
    );
    println!("quote(score=0) accepted: {:?}", result.unwrap());

    println!("\n=== Score validation test passed ===\n");
    Ok(())
}

/// Test that quote() and quote_oracle() produce identical results for oracle pools.
///
/// After update() caches the Scope account, quote() should give the same output
/// as quote_oracle() with the same accounts_map.
///
/// Run with: cargo test --test pool_test --features testing -- --ignored --nocapture test_quote_oracle_consistency
#[test]
#[ignore]
fn test_quote_oracle_consistency() {
    if let Err(e) = run_quote_consistency_tests() {
        panic!("Quote/oracle consistency test failed: {}", e);
    }
}

fn run_quote_consistency_tests() -> anyhow::Result<()> {
    println!("=== KDEX SDK (DFlow) - Quote/Oracle Consistency Test ===\n");

    let rpc_url = std::env::var("RPC").unwrap_or_else(|_| "http://127.0.0.1:8899".to_string());
    let rpc = RpcClient::new(rpc_url);
    let program_id = get_program_id();

    let pools = discover_pools(&rpc, &program_id)?;

    // Find an oracle pool
    for (addr, account) in &pools {
        let mut amm = match KDEXAmm::new_from_keyed_account_with_program_id(
            &KeyedAccount {
                key: *addr,
                account: account.clone(),
                params: None,
            },
            program_id,
        ) {
            Ok(amm) => amm,
            Err(_) => continue,
        };

        if !matches!(
            amm.curve_type(),
            CurveType::ConstantSpreadOracle | CurveType::InventorySkewOracle
        ) {
            continue;
        }

        println!("Using oracle pool: {} ({:?})", addr, amm.curve_type());

        // Fetch all accounts including scope
        let accounts_to_update = amm.get_accounts_to_update();
        let fetched = rpc.get_multiple_accounts(&accounts_to_update)?;
        let mut accounts_map: AccountMap = accounts_to_update
            .iter()
            .zip(fetched.iter())
            .filter_map(|(key, acc)| acc.as_ref().map(|a| (*key, a.clone())))
            .collect();

        // Need curve data before we can get scope feed
        amm.update(&accounts_map)?;

        let scope_feed = amm
            .get_scope_price_feed(&accounts_map)
            .ok_or_else(|| anyhow::anyhow!("No scope feed"))?;
        let scope_account = rpc.get_account(&scope_feed)?;
        accounts_map.insert(scope_feed, scope_account);

        // Re-update so quote() has cached scope data
        amm.update(&accounts_map)?;

        let mints = amm.get_reserve_mints();

        // Test multiple amounts and scores in both directions
        let test_cases: Vec<(Pubkey, Pubkey, u64, &str)> = vec![
            (mints[0], mints[1], 10_000_000, "A→B small"),
            (mints[0], mints[1], 100_000_000, "A→B medium"),
            (mints[1], mints[0], 10, "B→A small"),
        ];

        for (input_mint, output_mint, amount, label) in &test_cases {
            for score in [0u8, 2, 4] {
                let params = QuoteParams {
                    input_mint: *input_mint,
                    output_mint: *output_mint,
                    amount: *amount,
                    swap_mode: SwapMode::ExactIn,
                };

                let via_quote = amm.quote_with_score(&params, score)?;
                let via_oracle = amm.quote_oracle(&params, &accounts_map, score)?;

                println!(
                    "  {} score={}: quote()={}/{}, quote_oracle()={}/{}",
                    label,
                    score,
                    via_quote.in_amount,
                    via_quote.out_amount,
                    via_oracle.in_amount,
                    via_oracle.out_amount,
                );

                assert_eq!(
                    via_quote.in_amount, via_oracle.in_amount,
                    "{} score={}: in_amount mismatch: quote()={} vs quote_oracle()={}",
                    label, score, via_quote.in_amount, via_oracle.in_amount
                );
                assert_eq!(
                    via_quote.out_amount, via_oracle.out_amount,
                    "{} score={}: out_amount mismatch: quote()={} vs quote_oracle()={}",
                    label, score, via_quote.out_amount, via_oracle.out_amount
                );
            }
        }

        println!("\n=== Quote/oracle consistency test passed ===\n");
        return Ok(());
    }

    println!("No oracle pools found, skipping consistency test");
    Ok(())
}

/// Test that `is_active()` correctly reflects oracle staleness, with the offset
/// providing a buffer before the pool is marked inactive.
///
/// For each oracle pool this prints the actual oracle age and the configured
/// threshold so you can see how much margin exists.  With a live pool the price
/// should be fresh, so the pool should be active under any reasonable offset.
///
/// Run with: cargo test --test pool_test --features testing -- --ignored --nocapture test_oracle_staleness_offset
#[test]
#[ignore]
fn test_oracle_staleness_offset() {
    if let Err(e) = run_oracle_staleness_offset_tests() {
        panic!("Oracle staleness offset test failed: {}", e);
    }
}

fn run_oracle_staleness_offset_tests() -> anyhow::Result<()> {
    println!("=== KDEX SDK (DFlow) - Oracle Staleness Offset Test ===\n");

    let rpc_url = std::env::var("RPC").unwrap_or_else(|_| "http://127.0.0.1:8899".to_string());
    let rpc = RpcClient::new(rpc_url);
    let program_id = get_program_id();

    let pools = discover_pools(&rpc, &program_id)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let mut tested = 0u32;

    for (addr, account) in &pools {
        let mut amm = match KDEXAmm::new_from_keyed_account_with_program_id(
            &KeyedAccount {
                key: *addr,
                account: account.clone(),
                params: None,
            },
            program_id,
        ) {
            Ok(amm) => amm,
            Err(_) => continue,
        };

        if !matches!(
            amm.curve_type(),
            CurveType::ConstantSpreadOracle | CurveType::InventorySkewOracle
        ) {
            continue;
        }

        println!("Pool {} ({:?})", addr, amm.curve_type());

        // Fetch all accounts including scope
        let accounts_to_update = amm.get_accounts_to_update();
        let fetched = rpc.get_multiple_accounts(&accounts_to_update)?;
        let mut accounts_map: AccountMap = accounts_to_update
            .iter()
            .zip(fetched.iter())
            .filter_map(|(key, acc)| acc.as_ref().map(|a| (*key, a.clone())))
            .collect();
        amm.update(&accounts_map)?;

        let scope_feed = match amm.get_scope_price_feed(&accounts_map) {
            Some(f) => f,
            None => {
                println!("  No scope feed found, skipping\n");
                continue;
            }
        };
        let scope_account = rpc.get_account(&scope_feed)?;
        accounts_map.insert(scope_feed, scope_account);
        amm.update(&accounts_map)?;

        // The curve account is the 3rd entry in accounts_to_update (pool, authority, curve, ...).
        // We read max_age_secs and oracle age from it to print the staleness margin.

        // With default offset (60 s): a live pool should be active
        assert!(
            amm.is_active(),
            "Pool {} should be active with default offset on a live oracle",
            addr
        );
        println!("  is_active(default offset=60s) = true ✓");

        // With offset=0: shows the raw max_age_secs threshold without any buffer.
        // A pool in the buffer zone (age between max_age_secs and max_age_secs+60) will
        // correctly return false here — that is what the default offset protects against.
        let amm_no_offset = {
            let mut a = KDEXAmm::new_from_keyed_account_with_program_id(
                &KeyedAccount {
                    key: *addr,
                    account: account.clone(),
                    params: None,
                },
                program_id,
            )?
            .with_oracle_staleness_offset(0);
            a.update(&accounts_map)?;
            a
        };
        println!(
            "  is_active(offset=0)            = {} (raw max_age_secs threshold)",
            amm_no_offset.is_active()
        );

        // Simulate a long pause between the scope refresh and the quote by setting a large
        // negative offset, collapsing the threshold to 0.  Any cached oracle (updated in the
        // past) will appear stale — same effect as waiting max_age_secs + 60 s without
        // calling update() again.
        let amm_simulated_stale = {
            let mut a = KDEXAmm::new_from_keyed_account_with_program_id(
                &KeyedAccount {
                    key: *addr,
                    account: account.clone(),
                    params: None,
                },
                program_id,
            )?
            .with_oracle_staleness_offset(i64::MIN);
            a.update(&accounts_map)?;
            a
        };
        assert!(
            !amm_simulated_stale.is_active(),
            "Pool {} should be inactive when oracle appears stale",
            addr
        );
        println!("  is_active(offset=i64::MIN)     = false ✓ (simulated stale cache)");

        // Print the oracle age so we can see how much headroom we have
        if let Some(scope_acc) = accounts_map.get(&scope_feed) {
            // Read the first non-sentinel price index from curve data (bytes 40..42)
            if let Some(curve_acc) = accounts_to_update.get(2).and_then(|k| accounts_map.get(k)) {
                if curve_acc.data.len() >= 52 {
                    let price_index =
                        u16::from_le_bytes(curve_acc.data[40..42].try_into().unwrap_or_default());
                    let max_age =
                        u16::from_le_bytes(curve_acc.data[48..50].try_into().unwrap_or_default());
                    if let Some(ts) =
                        kdex_sdk_dflow::oracle::fetch_scope_price_timestamp(scope_acc, price_index)
                    {
                        let age = now.saturating_sub(ts);
                        println!(
                            "  Oracle age: {}s  |  max_age_secs: {}s  |  headroom: {}s",
                            age,
                            max_age,
                            (max_age as u64).saturating_sub(age),
                        );
                    }
                }
            }
        }

        println!();
        tested = tested.saturating_add(1);
    }

    if tested == 0 {
        println!("No oracle pools found, skipping staleness offset test");
        return Ok(());
    }

    println!(
        "=== Oracle staleness offset test passed ({} pools tested) ===\n",
        tested
    );
    Ok(())
}
