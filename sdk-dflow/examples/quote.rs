//! Example: Getting a quote from a Hyperplane pool using DFlow interface
//!
//! Run with: cargo run --example quote --features testing
//!
//! Environment variables:
//! - POOL: Pool address (can also be passed as CLI arg)
//! - RPC: RPC endpoint URL (default: http://127.0.0.1:8899)

use hyperplane::curve::base::CurveType;
use hyperplane_sdk_dflow::{AccountMap, Amm, HyperplaneAmm, KeyedAccount, QuoteParams, SwapMode};
use solana_client::rpc_client::RpcClient;
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;

fn main() -> anyhow::Result<()> {
    // Default: SOL/USDC pool on mainnet
    const DEFAULT_POOL: &str = "3uqKSr5gZzZSJXgrdikPeWGp1SnEqEayFABwzDQ3vRWe";

    // Get pool address from: 1) CLI arg, 2) $POOL env var, 3) default
    let pool_str = std::env::args()
        .nth(1)
        .or_else(|| std::env::var("POOL").ok())
        .unwrap_or_else(|| DEFAULT_POOL.to_string());

    let pool_address = Pubkey::from_str(&pool_str)?;

    println!("Fetching quote for Hyperplane pool (DFlow SDK)...");
    println!("Pool: {}", pool_address);

    // Create RPC client
    let rpc_url = std::env::var("RPC").unwrap_or_else(|_| "http://127.0.0.1:8899".to_string());
    let rpc = RpcClient::new(rpc_url);

    // Fetch pool account
    let pool_account = rpc.get_account(&pool_address)?;
    println!(
        "Pool account fetched, size: {} bytes",
        pool_account.data.len()
    );

    // Create AMM instance
    let mut amm = HyperplaneAmm::new_from_keyed_account(&KeyedAccount {
        key: pool_address,
        account: pool_account,
        params: None,
    })?;

    println!("Pool label: {}", amm.label());
    println!("Curve type: {:?}", amm.curve_type());
    println!("Withdrawals only: {}", amm.is_withdrawals_only());

    // Get reserve mints
    let mints = amm.get_reserve_mints();
    println!("Token A mint: {}", mints[0]);
    println!("Token B mint: {}", mints[1]);

    // Get accounts to update
    let accounts_to_update = amm.get_accounts_to_update();
    println!(
        "\nFetching {} accounts to update...",
        accounts_to_update.len()
    );

    // Fetch accounts
    let accounts = rpc.get_multiple_accounts(&accounts_to_update)?;
    let accounts_map: AccountMap = accounts_to_update
        .iter()
        .zip(accounts.iter())
        .filter_map(|(key, acc)| acc.as_ref().map(|a| (*key, a.clone())))
        .collect();

    println!("Updating AMM with fresh account data...");
    amm.update(&accounts_map)?;

    // Check if this is an oracle curve and fetch Scope price feed if needed
    let is_oracle = matches!(
        amm.curve_type(),
        CurveType::ConstantSpreadOracle | CurveType::InventorySkewOracle
    );

    // Extended accounts map for oracle pools
    let mut oracle_accounts_map = accounts_map.clone();

    if is_oracle {
        println!("Oracle curve detected - fetching Scope price feed...");

        // Get the Scope price feed address from the curve
        let scope_price_feed = amm
            .get_scope_price_feed(&accounts_map)
            .ok_or_else(|| anyhow::anyhow!("Failed to get Scope price feed from curve"))?;

        println!("  Scope price feed: {}", scope_price_feed);

        // Fetch the Scope price feed account
        let scope_account = rpc.get_account(&scope_price_feed)?;
        oracle_accounts_map.insert(scope_price_feed, scope_account);
        println!("  Scope price feed account fetched");
    }

    // Get quote for 1 SOL -> USDC
    let in_amount = 1_000_000_000; // 1 SOL
    println!("\nGetting quote for {} SOL...", in_amount as f64 / 1e9);

    let quote = if is_oracle {
        amm.quote_oracle(
            &QuoteParams {
                input_mint: mints[0],
                output_mint: mints[1],
                amount: in_amount,
                swap_mode: SwapMode::ExactIn,
            },
            &oracle_accounts_map,
        )?
    } else {
        amm.quote(&QuoteParams {
            input_mint: mints[0],
            output_mint: mints[1],
            amount: in_amount,
            swap_mode: SwapMode::ExactIn,
        })?
    };

    // Note: DFlow Quote only contains in_amount and out_amount
    println!("\nQuote Result:");
    println!("  Input:  {} SOL", quote.in_amount as f64 / 1e9);
    println!("  Output: {} USDC", quote.out_amount as f64 / 1e6);
    println!(
        "  Price:  {} USDC/SOL",
        (quote.out_amount as f64 / 1e6) / (quote.in_amount as f64 / 1e9)
    );

    // Get reverse quote: USDC -> SOL
    let in_amount = quote.out_amount;
    println!(
        "\nGetting reverse quote for {} USDC...",
        in_amount as f64 / 1e6
    );

    let reverse_quote = if is_oracle {
        amm.quote_oracle(
            &QuoteParams {
                input_mint: mints[1],
                output_mint: mints[0],
                amount: in_amount,
                swap_mode: SwapMode::ExactIn,
            },
            &oracle_accounts_map,
        )?
    } else {
        amm.quote(&QuoteParams {
            input_mint: mints[1],
            output_mint: mints[0],
            amount: in_amount,
            swap_mode: SwapMode::ExactIn,
        })?
    };

    println!("\nReverse Quote Result:");
    println!("  Input:  {} USDC", reverse_quote.in_amount as f64 / 1e6);
    println!("  Output: {} SOL", reverse_quote.out_amount as f64 / 1e9);

    Ok(())
}
