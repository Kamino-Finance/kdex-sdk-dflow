//! Oracle price fetching and calculation support for oracle-based curves
//!
//! This module re-exports shared oracle functionality from kdex-client with
//! error conversion to SDK-specific error types.

use crate::error::{Result, SdkError};
use kdex_client::generated::types::Fees;
use kdex_client::TradeDirection;
use solana_sdk::account::Account;

// Re-export types from kdex-client
pub use kdex_client::TradeDirection as ReexportedTradeDirection;

/// Fetches a single Scope oracle price from the price feed account
pub fn fetch_scope_price(price_feed_account: &Account, price_index: u16) -> Result<(u64, u64)> {
    kdex_client::oracle::fetch_scope_price(price_feed_account, price_index)
        .map_err(oracle_error_to_sdk_error)
}

/// Fetches and multiplies prices from a Scope oracle price chain
pub fn fetch_scope_price_chain(
    price_feed_account: &Account,
    price_chain: &[u16; 4],
) -> Result<(u128, u64)> {
    kdex_client::oracle::fetch_scope_price_chain(price_feed_account, price_chain)
        .map_err(oracle_error_to_sdk_error)
}

/// Calculate quote for ConstantSpreadOracle curve
#[allow(clippy::too_many_arguments)]
pub fn calculate_constant_spread_quote(
    fees: &Fees,
    amount_in: u64,
    destination_vault_amount: u64,
    trade_direction: TradeDirection,
    curve_data: &[u8],
    scope_price_feed_account: &Account,
    token_a_decimals: u8,
    token_b_decimals: u8,
) -> Result<(u64, u64, u64)> {
    kdex_client::oracle::calculate_constant_spread_quote(
        fees,
        amount_in,
        destination_vault_amount,
        trade_direction,
        curve_data,
        scope_price_feed_account,
        token_a_decimals,
        token_b_decimals,
    )
    .map_err(oracle_error_to_sdk_error)
}

/// Calculate quote for InventorySkewOracle curve
#[allow(clippy::too_many_arguments)]
pub fn calculate_inventory_skew_quote(
    fees: &Fees,
    amount_in: u64,
    source_vault_amount: u64,
    destination_vault_amount: u64,
    trade_direction: TradeDirection,
    curve_data: &[u8],
    scope_price_feed_account: &Account,
    token_a_decimals: u8,
    token_b_decimals: u8,
) -> Result<(u64, u64, u64)> {
    kdex_client::oracle::calculate_inventory_skew_quote(
        fees,
        amount_in,
        source_vault_amount,
        destination_vault_amount,
        trade_direction,
        curve_data,
        scope_price_feed_account,
        token_a_decimals,
        token_b_decimals,
    )
    .map_err(oracle_error_to_sdk_error)
}

/// Convert kdex-client OracleError to SDK-specific SdkError
fn oracle_error_to_sdk_error(err: kdex_client::oracle::OracleError) -> SdkError {
    match err {
        kdex_client::oracle::OracleError::InvalidOracleConfig(msg) => {
            SdkError::InvalidOracleConfig(msg)
        }
        kdex_client::oracle::OracleError::CurveError(msg) => SdkError::CurveError(msg),
        kdex_client::oracle::OracleError::DeserializationError(msg) => {
            SdkError::DeserializationError(msg)
        }
        kdex_client::oracle::OracleError::InsufficientLiquidity {
            required,
            available,
        } => SdkError::InsufficientLiquidity {
            required,
            available,
        },
    }
}
