//! Off-chain quote calculation for Hyperplane pools
//!
//! This module provides quote functionality that replicates on-chain swap calculations
//! without requiring an on-chain transaction. It supports all curve types including
//! oracle-based curves that require Scope price feeds.

use anchor_lang::AccountDeserialize;
use anchor_spl::token::TokenAccount;
use anyhow::{anyhow, Result};
use hyperplane::{
    curve::{
        base::{CurveType, SwapCurve},
        calculator::TradeDirection,
        fees::Fees,
    },
    state::{SwapPool, SwapState},
};
use orbit_link::async_client::AsyncClient;
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

/// Quote calculator for Hyperplane pools
pub struct QuoteCalculator<'a, T: AsyncClient> {
    client: &'a T,
}

impl<'a, T: AsyncClient> QuoteCalculator<'a, T> {
    /// Create a new quote calculator
    pub fn new(client: &'a T) -> Self {
        Self { client }
    }

    /// Get a quote for swapping tokens through a Hyperplane pool
    ///
    /// # Arguments
    /// * `pool_address` - Address of the Hyperplane swap pool
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

        // 3. Fetch vault balances
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

        // 4. Get curve type
        let curve_type = pool.curve_type();

        // 5. Calculate based on curve type
        let quote = match curve_type {
            CurveType::ConstantProduct
            | CurveType::ConstantPrice
            | CurveType::Offset
            | CurveType::Stable => {
                // Standard curves: fetch curve account and use the curve's swap method
                let curve_account = self.client.get_account(&pool.swap_curve).await?;

                self.calculate_standard_quote(
                    &pool,
                    amount_in,
                    source_vault_token.amount,
                    dest_vault_token.amount,
                    trade_direction,
                    &curve_account.data,
                )?
            }
            CurveType::ConstantSpreadOracle => {
                // Need to fetch curve account and Scope price
                let curve_account = self.client.get_account(&pool.swap_curve).await?;

                self.calculate_constant_spread_quote(
                    &pool.fees,
                    amount_in,
                    dest_vault_token.amount,
                    trade_direction,
                    &curve_account.data,
                )
                .await?
            }
            CurveType::InventorySkewOracle => {
                // Need to fetch curve account and Scope price
                let curve_account = self.client.get_account(&pool.swap_curve).await?;

                self.calculate_inventory_skew_quote(
                    &pool.fees,
                    amount_in,
                    source_vault_token.amount,
                    dest_vault_token.amount,
                    trade_direction,
                    &curve_account.data,
                )
                .await?
            }
        };

        Ok(quote)
    }

    /// Calculate quote for standard (non-oracle) curves
    fn calculate_standard_quote(
        &self,
        pool: &SwapPool,
        amount_in: u64,
        pool_source_amount: u64,
        pool_destination_amount: u64,
        trade_direction: TradeDirection,
        curve_data: &[u8],
    ) -> Result<Quote> {
        // Deserialize curve based on curve type
        let curve: SwapCurve = match pool.curve_type() {
            CurveType::ConstantProduct => {
                let mut data = curve_data;
                let calculator: hyperplane::state::ConstantProductCurve =
                    hyperplane::state::ConstantProductCurve::try_deserialize(&mut data)?;
                SwapCurve {
                    calculator: std::sync::Arc::new(calculator),
                    curve_type: pool.curve_type(),
                }
            }
            CurveType::ConstantPrice => {
                let mut data = curve_data;
                let calculator: hyperplane::state::ConstantPriceCurve =
                    hyperplane::state::ConstantPriceCurve::try_deserialize(&mut data)?;
                SwapCurve {
                    calculator: std::sync::Arc::new(calculator),
                    curve_type: pool.curve_type(),
                }
            }
            CurveType::Offset => {
                let mut data = curve_data;
                let calculator: hyperplane::state::OffsetCurve =
                    hyperplane::state::OffsetCurve::try_deserialize(&mut data)?;
                SwapCurve {
                    calculator: std::sync::Arc::new(calculator),
                    curve_type: pool.curve_type(),
                }
            }
            CurveType::Stable => {
                let mut data = curve_data;
                let calculator: hyperplane::state::StableCurve =
                    hyperplane::state::StableCurve::try_deserialize(&mut data)?;
                SwapCurve {
                    calculator: std::sync::Arc::new(calculator),
                    curve_type: pool.curve_type(),
                }
            }
            _ => return Err(anyhow!("Unexpected curve type in calculate_standard_quote")),
        };

        // Use the standard curve.swap() method
        let swap_result = curve.swap(
            u128::from(amount_in),
            u128::from(pool_source_amount),
            u128::from(pool_destination_amount),
            trade_direction,
            &pool.fees,
        )?;

        Ok(Quote {
            in_amount: swap_result.source_amount_swapped as u64,
            out_amount: swap_result.destination_amount_swapped as u64,
            total_fees: swap_result.total_fees as u64,
        })
    }

    /// Calculate quote for ConstantSpreadOracle curve
    async fn calculate_constant_spread_quote(
        &self,
        fees: &Fees,
        amount_in: u64,
        destination_vault_amount: u64,
        trade_direction: TradeDirection,
        curve_data: &[u8],
    ) -> Result<Quote> {
        // Deserialize the full ConstantSpreadOracleCurve to access all parameters
        let mut curve_account_data = curve_data;
        let curve: hyperplane::state::ConstantSpreadOracleCurve =
            hyperplane::state::ConstantSpreadOracleCurve::try_deserialize(&mut curve_account_data)?;

        // Fetch Scope price chain
        let (price_value, price_exp) = self
            .fetch_scope_price_chain(curve.scope_price_feed, &curve.price_chain)
            .await?;

        // Calculate fees (same as on-chain)
        let trade_fee = fees.trading_fee(amount_in as u128)?;
        let owner_fee = fees.owner_trading_fee(amount_in as u128)?;
        let total_fees = trade_fee
            .checked_add(owner_fee)
            .ok_or_else(|| anyhow!("Fee calculation overflow"))?;
        let source_amount_less_fees =
            (amount_in as u128).checked_sub(total_fees).ok_or_else(|| {
                anyhow!(
                    "Amount too small to cover fees. Amount: {}, Fees: {}",
                    amount_in,
                    total_fees
                )
            })?;

        // Calculate swap using ConstantSpread logic
        let (source_amount_swapped, destination_amount_swapped) =
            hyperplane::curve::oracle::calculate_constant_spread_swap(
                source_amount_less_fees,
                price_value as u64,
                price_exp as u64,
                curve.bps_from_oracle,
                trade_direction,
            )?;

        // Check if there's sufficient liquidity in the destination vault
        if destination_amount_swapped > destination_vault_amount as u128 {
            return Err(anyhow!(
                "Insufficient liquidity. Required: {}, Available: {}",
                destination_amount_swapped,
                destination_vault_amount
            ));
        }

        Ok(Quote {
            in_amount: source_amount_swapped as u64,
            out_amount: destination_amount_swapped as u64,
            total_fees: total_fees as u64,
        })
    }

    /// Calculate quote for InventorySkewOracle curve
    async fn calculate_inventory_skew_quote(
        &self,
        fees: &Fees,
        amount_in: u64,
        source_vault_amount: u64,
        destination_vault_amount: u64,
        trade_direction: TradeDirection,
        curve_data: &[u8],
    ) -> Result<Quote> {
        // Parse curve account to get all parameters
        let mut curve_account_data = curve_data;
        let curve: hyperplane::state::InventorySkewOracleCurve =
            hyperplane::state::InventorySkewOracleCurve::try_deserialize(&mut curve_account_data)?;

        // Fetch Scope price chain
        let (price_value, price_exp) = self
            .fetch_scope_price_chain(curve.scope_price_feed, &curve.price_chain)
            .await?;

        // Calculate fees (same as on-chain)
        let trade_fee = fees.trading_fee(amount_in as u128)?;
        let owner_fee = fees.owner_trading_fee(amount_in as u128)?;
        let total_fees = trade_fee
            .checked_add(owner_fee)
            .ok_or_else(|| anyhow!("Fee calculation overflow"))?;
        let source_amount_less_fees =
            (amount_in as u128).checked_sub(total_fees).ok_or_else(|| {
                anyhow!(
                    "Amount too small to cover fees. Amount: {}, Fees: {}",
                    amount_in,
                    total_fees
                )
            })?;

        // Calculate swap using InventorySkew logic
        let (source_amount_swapped, destination_amount_swapped) =
            hyperplane::curve::oracle::calculate_inventory_swap_amounts(
                source_amount_less_fees,
                price_value as u64,
                price_exp as u64,
                trade_direction,
                source_vault_amount as u128,
                &curve,
            )?;

        // Check if there's sufficient liquidity in the destination vault
        if destination_amount_swapped > destination_vault_amount as u128 {
            return Err(anyhow!(
                "Insufficient liquidity. Required: {}, Available: {}",
                destination_amount_swapped,
                destination_vault_amount
            ));
        }

        Ok(Quote {
            in_amount: source_amount_swapped as u64,
            out_amount: destination_amount_swapped as u64,
            total_fees: total_fees as u64,
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

    /// Fetch current price from Scope oracle
    async fn fetch_scope_price(&self, price_feed: Pubkey, price_index: u16) -> Result<(u64, i32)> {
        // Validate price index is within bounds
        if (price_index as usize) >= scope_types::MAX_ENTRIES {
            return Err(anyhow!(
                "Price index {} out of bounds (max: {})",
                price_index,
                scope_types::MAX_ENTRIES
            ));
        }

        // Fetch the Scope OraclePrices account
        let price_feed_account = self.client.get_account(&price_feed).await?;

        // Validate account owner is the Scope program
        if price_feed_account.owner != scope_types::id() {
            return Err(anyhow!(
                "Invalid Scope account owner. Expected: {}, Got: {}",
                scope_types::id(),
                price_feed_account.owner
            ));
        }

        // OraclePrices layout: discriminator(8) + oracle_mappings(32) + prices[512]
        let dated_price_size = std::mem::size_of::<scope_types::DatedPrice>();
        let offset = 8_usize
            .checked_add(32)
            .and_then(|base| {
                (price_index as usize)
                    .checked_mul(dated_price_size)
                    .and_then(|product| base.checked_add(product))
            })
            .ok_or_else(|| anyhow!("Offset calculation overflow"))?;

        let end_offset = offset
            .checked_add(dated_price_size)
            .ok_or_else(|| anyhow!("End offset calculation overflow"))?;

        if price_feed_account.data.len() < end_offset {
            return Err(anyhow!(
                "Account data too short for price index {}",
                price_index
            ));
        }

        // Deserialize the DatedPrice at the specified index
        let dated_price: &scope_types::DatedPrice =
            bytemuck::from_bytes(&price_feed_account.data[offset..end_offset]);

        Ok((dated_price.price.value, dated_price.price.exp as i32))
    }

    /// Fetch and multiply prices from a Scope oracle price chain
    async fn fetch_scope_price_chain(
        &self,
        price_feed: Pubkey,
        price_chain: &[u16; 4],
    ) -> Result<(u128, i32)> {
        use hyperplane::curve::oracle::utils::PRICE_CHAIN_TERMINATOR;

        // Count valid indices in chain
        let chain_len = price_chain
            .iter()
            .take_while(|&&idx| idx != PRICE_CHAIN_TERMINATOR)
            .count();

        if chain_len == 0 {
            return Err(anyhow!("Price chain is empty"));
        }

        // Fetch first price
        let (first_value, first_exp) = self.fetch_scope_price(price_feed, price_chain[0]).await?;
        let mut combined_value = first_value as u128;
        let mut combined_exp = first_exp;

        // Multiply remaining prices in the chain
        for &price_index in price_chain.iter().skip(1).take(chain_len.saturating_sub(1)) {
            let (value, exp) = self.fetch_scope_price(price_feed, price_index).await?;
            combined_value = combined_value
                .checked_mul(value as u128)
                .ok_or_else(|| anyhow!("Price multiplication overflow"))?;
            combined_exp = combined_exp
                .checked_add(exp)
                .ok_or_else(|| anyhow!("Exponent addition overflow"))?;
        }

        Ok((combined_value, combined_exp))
    }
}
