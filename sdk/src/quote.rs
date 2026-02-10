//! Off-chain quote calculation for KDEX pools
//!
//! This module provides quote functionality that replicates on-chain swap calculations
//! without requiring an on-chain transaction. It supports all curve types including
//! oracle-based curves that require Scope price feeds.

use anchor_lang::AccountDeserialize;
use anchor_spl::token::TokenAccount;
use anyhow::{anyhow, Result};
use kdex_client::generated::accounts::SwapPool;
use kdex_client::state::SwapState;
use kdex_client::{CurveType, TradeDirection};
use solana_sdk::pubkey::Pubkey;

/// Result of a quote calculation
#[derive(Debug, Clone)]
pub struct Quote {
    /// Amount of tokens that will be consumed
    pub in_amount: u64,
    /// Amount of tokens that will be received
    pub out_amount: u64,
    /// Total fees charged (in source token)
    pub total_fees: u64,
}

/// Quote calculator for KDEX pools
#[cfg(feature = "rpc-client")]
pub struct QuoteCalculator<'a> {
    client: &'a solana_client::nonblocking::rpc_client::RpcClient,
}

#[cfg(feature = "rpc-client")]
impl<'a> QuoteCalculator<'a> {
    /// Create a new quote calculator
    pub fn new(client: &'a solana_client::nonblocking::rpc_client::RpcClient) -> Self {
        Self { client }
    }

    /// Get a quote for swapping tokens through a KDEX pool
    ///
    /// # Arguments
    /// * `pool_address` - Address of the KDEX swap pool
    /// * `trade_direction` - Direction of the trade (AtoB = buy token B with A, BtoA = sell token B for A)
    /// * `amount_in` - Amount of input tokens (in native units, e.g., lamports)
    ///
    /// # Returns
    /// A `Quote` containing the expected output amount, fees, and slippage protection
    pub async fn get_quote(
        &self,
        pool_address: Pubkey,
        trade_direction: TradeDirection,
        amount_in: u64,
    ) -> Result<Quote> {
        // 1. Fetch pool account
        let pool_account = self.client.get_account(&pool_address).await?;
        let mut pool_data = pool_account.data.as_slice();
        let pool: SwapPool = SwapPool::try_deserialize(&mut pool_data)?;

        // 2. Fetch vault balances
        let (source_vault, dest_vault) = match trade_direction {
            TradeDirection::AtoB => (pool.token_a_vault, pool.token_b_vault),
            TradeDirection::BtoA => (pool.token_b_vault, pool.token_a_vault),
        };

        let source_vault_account = self.client.get_account(&source_vault).await?;
        let mut source_vault_data = &source_vault_account.data[..];
        let source_vault_token: TokenAccount =
            TokenAccount::try_deserialize(&mut source_vault_data)?;

        let dest_vault_account = self.client.get_account(&dest_vault).await?;
        let mut dest_vault_data = &dest_vault_account.data[..];
        let dest_vault_token: TokenAccount = TokenAccount::try_deserialize(&mut dest_vault_data)?;

        // 3. Get curve type
        let curve_type = pool.curve_type();

        // 4. Calculate based on curve type
        let quote = match curve_type {
            CurveType::ConstantProduct
            | CurveType::ConstantPrice
            | CurveType::Offset
            | CurveType::Stable => {
                // Standard curves: fetch curve account and use kdex_client::quote
                let curve_account = self.client.get_account(&pool.swap_curve).await?;

                let result = kdex_client::quote::calculate_quote(
                    curve_type,
                    &pool.fees,
                    amount_in,
                    source_vault_token.amount,
                    dest_vault_token.amount,
                    trade_direction,
                    &curve_account.data,
                )
                .map_err(|e| anyhow!("Quote calculation failed: {:?}", e))?;

                Quote {
                    in_amount: result.source_amount_swapped,
                    out_amount: result.destination_amount_swapped,
                    total_fees: result.total_fees,
                }
            }
            CurveType::ConstantSpreadOracle => {
                // Need to fetch curve account, Scope price, and mint decimals
                let curve_account = self.client.get_account(&pool.swap_curve).await?;
                let token_a_mint_account = self.client.get_account(&pool.token_a_mint).await?;
                let token_b_mint_account = self.client.get_account(&pool.token_b_mint).await?;
                let token_a_decimals = *token_a_mint_account.data.get(44).unwrap_or(&0);
                let token_b_decimals = *token_b_mint_account.data.get(44).unwrap_or(&0);

                self.calculate_constant_spread_quote(
                    &pool.fees,
                    amount_in,
                    dest_vault_token.amount,
                    trade_direction,
                    &curve_account.data,
                    token_a_decimals,
                    token_b_decimals,
                )
                .await?
            }
            CurveType::InventorySkewOracle => {
                // Need to fetch curve account, Scope price, and mint decimals
                let curve_account = self.client.get_account(&pool.swap_curve).await?;
                let token_a_mint_account = self.client.get_account(&pool.token_a_mint).await?;
                let token_b_mint_account = self.client.get_account(&pool.token_b_mint).await?;
                let token_a_decimals = *token_a_mint_account.data.get(44).unwrap_or(&0);
                let token_b_decimals = *token_b_mint_account.data.get(44).unwrap_or(&0);

                self.calculate_inventory_skew_quote(
                    &pool.fees,
                    amount_in,
                    source_vault_token.amount,
                    dest_vault_token.amount,
                    trade_direction,
                    &curve_account.data,
                    token_a_decimals,
                    token_b_decimals,
                )
                .await?
            }
        };

        Ok(quote)
    }

    /// Calculate quote for ConstantSpreadOracle curve
    #[allow(clippy::too_many_arguments)]
    async fn calculate_constant_spread_quote(
        &self,
        fees: &kdex_client::generated::types::Fees,
        amount_in: u64,
        destination_vault_amount: u64,
        trade_direction: TradeDirection,
        curve_data: &[u8],
        token_a_decimals: u8,
        token_b_decimals: u8,
    ) -> Result<Quote> {
        // Deserialize the full ConstantSpreadOracleCurve to access all parameters
        let mut data = curve_data;
        let curve: kdex_client::curves::ConstantSpreadOracleCurve =
            anchor_lang::AccountDeserialize::try_deserialize(&mut data)
                .map_err(|e| anyhow!("Failed to deserialize curve: {}", e))?;

        // Fetch Scope price feed account
        let scope_account = self.client.get_account(&curve.scope_price_feed).await?;

        // Use kdex_client oracle to calculate quote
        let (in_amount, out_amount, total_fees) =
            kdex_client::oracle::calculate_constant_spread_quote(
                fees,
                amount_in,
                destination_vault_amount,
                trade_direction,
                curve_data,
                &scope_account,
                token_a_decimals,
                token_b_decimals,
            )
            .map_err(|e| anyhow!("Oracle quote calculation failed: {:?}", e))?;

        Ok(Quote {
            in_amount,
            out_amount,
            total_fees,
        })
    }

    /// Calculate quote for InventorySkewOracle curve
    #[allow(clippy::too_many_arguments)]
    async fn calculate_inventory_skew_quote(
        &self,
        fees: &kdex_client::generated::types::Fees,
        amount_in: u64,
        source_vault_amount: u64,
        destination_vault_amount: u64,
        trade_direction: TradeDirection,
        curve_data: &[u8],
        token_a_decimals: u8,
        token_b_decimals: u8,
    ) -> Result<Quote> {
        // Deserialize the full InventorySkewOracleCurve to access all parameters
        let mut data = curve_data;
        let curve: kdex_client::curves::InventorySkewOracleCurve =
            anchor_lang::AccountDeserialize::try_deserialize(&mut data)
                .map_err(|e| anyhow!("Failed to deserialize curve: {}", e))?;

        // Fetch Scope price feed account
        let scope_account = self.client.get_account(&curve.scope_price_feed).await?;

        // Use kdex_client oracle to calculate quote
        let (in_amount, out_amount, total_fees) =
            kdex_client::oracle::calculate_inventory_skew_quote(
                fees,
                amount_in,
                source_vault_amount,
                destination_vault_amount,
                trade_direction,
                curve_data,
                &scope_account,
                token_a_decimals,
                token_b_decimals,
            )
            .map_err(|e| anyhow!("Oracle quote calculation failed: {:?}", e))?;

        Ok(Quote {
            in_amount,
            out_amount,
            total_fees,
        })
    }

    /// Get a quote by specifying the input mint (convenience method)
    ///
    /// This method automatically determines the trade direction based on which mint
    /// matches the input. Use `get_quote()` if you want explicit control over direction.
    pub async fn get_quote_by_mint(
        &self,
        pool_address: Pubkey,
        input_mint: Pubkey,
        amount_in: u64,
    ) -> Result<Quote> {
        // Fetch pool to determine direction
        let pool_account = self.client.get_account(&pool_address).await?;
        let mut pool_data = pool_account.data.as_slice();
        let pool: SwapPool = SwapPool::try_deserialize(&mut pool_data)?;

        // Infer trade direction from input mint
        let trade_direction = if input_mint == pool.token_a_mint {
            TradeDirection::AtoB
        } else if input_mint == pool.token_b_mint {
            TradeDirection::BtoA
        } else {
            return Err(anyhow!(
                "Input mint {} does not match pool mints (A: {}, B: {})",
                input_mint,
                pool.token_a_mint,
                pool.token_b_mint
            ));
        };

        // Call main quote method
        self.get_quote(pool_address, trade_direction, amount_in)
            .await
    }
}
