//! AMM implementation for KDEX using the DFlow AMM interface
//!
//! This module provides a DFlow-compatible AMM implementation for interacting with
//! KDEX pools, supporting all curve types including oracle-based curves.

use anchor_lang::AccountDeserialize;
use anyhow::Result;
use kdex_client::curves::{ConstantSpreadOracleCurve, InventorySkewOracleCurve};
use kdex_client::generated::accounts::SwapPool;
use kdex_client::liquidity::{
    cap_input_proportional, estimate_max_input_for_vault, search_max_input, SwapFit,
};
use kdex_client::state::SwapState;
use kdex_client::{CurveType, TradeDirection, KDEX_ID};
use solana_sdk::pubkey::Pubkey;
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use crate::oracle;

/// Maximum allowed score value for spread widening
const MAX_SCORE: u8 = 4;

// Re-export dflow-amm-interface types for convenience
pub use anchor_spl::token::TokenAccount;
pub use dflow_amm_interface::{
    AccountMap, Amm, AmmContext, KeyedAccount, Quote, QuoteParams, Swap, SwapAndAccountMetas,
    SwapMode, SwapParams,
};

/// KDEX AMM implementation compatible with DFlow
///
/// # DFlow Compatibility
///
/// Fully implements the DFlow `Amm` trait. The standard `quote()` method works for all curve types,
/// including oracle curves.
///
/// ## Oracle Curves (ConstantSpreadOracle, InventorySkewOracle)
///
/// Oracle curves use Scope price feeds. When you call `update()` with the Scope account included,
/// it's cached internally so `quote()` can access it (since the DFlow trait doesn't allow
/// passing accounts to `quote()`).
///
/// ### Standard flow (DFlow compatible):
/// ```ignore
/// // 1. Get accounts to update (vaults + curve)
/// let accounts_to_update = amm.get_accounts_to_update();
///
/// // 2. For oracle curves, add Scope price feed
/// if matches!(amm.curve_type(), CurveType::ConstantSpreadOracle | CurveType::InventorySkewOracle) {
///     let scope_feed = amm.get_scope_price_feed(&accounts_map)?;
///     accounts_to_update.push(scope_feed);
/// }
///
/// // 3. Fetch all accounts
/// let accounts = rpc.get_multiple_accounts(&accounts_to_update)?;
/// let accounts_map = /* build map */;
///
/// // 4. Update (caches Scope account for oracle curves)
/// amm.update(&accounts_map)?;
///
/// // 5. Quote works for all curve types
/// let quote = amm.quote(&params)?;
/// ```
#[derive(Clone, Debug)]
pub struct KDEXAmm {
    /// The pool's public key
    pool_key: Pubkey,
    /// The swap pool account data
    pool: SwapPool,
    /// Token A vault account (updated via update())
    token_a_vault: Option<TokenAccount>,
    /// Token B vault account (updated via update())
    token_b_vault: Option<TokenAccount>,
    /// Label for this AMM
    label: String,
    /// Program ID
    program_id: Pubkey,
    /// Token program for token A (detected from vault owner)
    token_a_program: Option<Pubkey>,
    /// Token program for token B (detected from vault owner)
    token_b_program: Option<Pubkey>,
    /// Cached account data hashes (for change detection)
    account_hashes: HashMap<Pubkey, u64>,
    /// Cached curve account data (for oracle quotes)
    curve_account_data: Option<Vec<u8>>,
    /// Scope price feed accounts (for oracle curves, cached during update())
    /// Keyed by Scope feed pubkey to support multiple oracle accounts
    scope_price_feeds: HashMap<Pubkey, solana_sdk::account::Account>,
    /// Token A decimals (populated from mint account during update)
    token_a_decimals: u8,
    /// Token B decimals (populated from mint account during update)
    token_b_decimals: u8,
    /// Vault capacity strategy for handling swaps that exceed liquidity.
    /// - `0`: No preemptive cap; quote the full amount and binary search on overflow.
    /// - `9000-9999`: Preemptively estimate max input targeting this % of vault (default 9800 = 98%).
    vault_capacity_target_bps: u16,
    /// Extra seconds added to the curve's `max_age_secs` when checking oracle staleness
    /// off-chain. `None` = auto: use `min(max_age_secs)` across the active price chain.
    /// `Some(x)` = fixed override (can be negative to tighten, useful for testing).
    oracle_staleness_offset_secs: Option<i64>,
    /// Whether to check oracle staleness in `is_active()`. Default: true.
    oracle_staleness_check: bool,
}

impl KDEXAmm {
    /// Creates a new KDEXAmm from a keyed account
    pub fn new_from_keyed_account(keyed_account: &KeyedAccount) -> Result<Self> {
        let pool: SwapPool =
            AccountDeserialize::try_deserialize(&mut keyed_account.account.data.as_ref())?;

        Ok(Self {
            pool_key: keyed_account.key,
            label: "KDEX".into(),
            program_id: KDEX_ID,
            pool,
            token_a_vault: None,
            token_b_vault: None,
            token_a_program: None,
            token_b_program: None,
            account_hashes: HashMap::new(),
            curve_account_data: None,
            scope_price_feeds: HashMap::new(),
            token_a_decimals: 0,
            token_b_decimals: 0,
            vault_capacity_target_bps: 9800, // Default: 98%
            oracle_staleness_offset_secs: None,
            oracle_staleness_check: true,
        })
    }

    /// Creates a new KDEXAmm with a custom program ID (for testing or devnet)
    pub fn new_from_keyed_account_with_program_id(
        keyed_account: &KeyedAccount,
        program_id: Pubkey,
    ) -> Result<Self> {
        // Validate account owner matches provided program ID
        if keyed_account.account.owner != program_id {
            anyhow::bail!(
                "Invalid account owner: expected {}, got {}",
                program_id,
                keyed_account.account.owner
            );
        }

        let pool: SwapPool =
            AccountDeserialize::try_deserialize(&mut keyed_account.account.data.as_ref())?;

        Ok(Self {
            pool_key: keyed_account.key,
            label: "KDEX".into(),
            program_id,
            pool,
            token_a_vault: None,
            token_b_vault: None,
            token_a_program: None,
            token_b_program: None,
            account_hashes: HashMap::new(),
            curve_account_data: None,
            scope_price_feeds: HashMap::new(),
            token_a_decimals: 0,
            token_b_decimals: 0,
            vault_capacity_target_bps: 9800, // Default: 98%
            oracle_staleness_offset_secs: None,
            oracle_staleness_check: true,
        })
    }

    /// Sets the target vault capacity in basis points (default: 9800 = 98%)
    ///
    /// This controls how much of the vault capacity can be used in a single swap.
    /// Lower values are more conservative (higher safety buffer), higher values
    /// allow more capital efficiency but with less margin for error.
    ///
    /// Special value `0` disables preemptive capacity estimation entirely and uses
    /// binary search to find the maximum input that fits when liquidity is exceeded.
    ///
    /// # Arguments
    /// * `target_bps` - Target capacity in basis points. Either `0` (binary search)
    ///   or `9000-9999` (90%-99.99% preemptive estimation).
    ///
    /// # Example
    /// ```ignore
    /// use kdex_sdk_dflow::{KDEXAmm, KeyedAccount};
    ///
    /// // No cap - use binary search on overflow (most accurate, more iterations)
    /// let uncapped_amm = KDEXAmm::new_from_keyed_account(&keyed_account)?
    ///     .with_vault_capacity_target(0)?;
    ///
    /// // Conservative routing strategy - use 95% of vault capacity
    /// let conservative_amm = KDEXAmm::new_from_keyed_account(&keyed_account)?
    ///     .with_vault_capacity_target(9500)?;
    ///
    /// // Default behavior (98%) - no configuration needed
    /// let default_amm = KDEXAmm::new_from_keyed_account(&keyed_account)?;
    /// ```
    ///
    /// # Errors
    /// Returns an error if `target_bps` is outside the valid range.
    pub fn with_vault_capacity_target(mut self, target_bps: u16) -> Result<Self> {
        if target_bps != 0 && !(9000..=9999).contains(&target_bps) {
            anyhow::bail!(
                "vault_capacity_target_bps must be 0 (binary search) or 9000-9999 (90%-99.99%), got {}",
                target_bps
            );
        }
        self.vault_capacity_target_bps = target_bps;
        Ok(self)
    }

    /// Sets the oracle staleness offset in seconds (default: 60).
    ///
    /// This is added to the curve's `max_age_secs` when checking oracle staleness
    /// off-chain, providing a buffer for cache lag and clock drift before marking
    /// the pool inactive.
    pub fn with_oracle_staleness_offset(mut self, offset_secs: i64) -> Self {
        self.oracle_staleness_offset_secs = Some(offset_secs);
        self
    }

    /// Enables or disables the oracle staleness check in `is_active()` (default: enabled).
    pub fn with_oracle_staleness_check(mut self, enabled: bool) -> Self {
        self.oracle_staleness_check = enabled;
        self
    }

    /// Returns the pool's `score_factor_bps` from cached curve data.
    ///
    /// Returns 0 for non-oracle curves or if curve data hasn't been cached yet.
    pub fn score_factor_bps(&self) -> u64 {
        self.read_score_factor_from_curve_data().unwrap_or(0)
    }

    /// Computes `score * score_factor_bps` from the cached curve data.
    ///
    /// Returns 0 when score is 0 or when curve data is unavailable/too short.
    fn score_multiplier_bps(&self, score: u8) -> u64 {
        if score == 0 {
            return 0;
        }
        let score_factor_bps = self.read_score_factor_from_curve_data().unwrap_or(0);
        (score as u64).saturating_mul(score_factor_bps)
    }

    /// Reads `score_factor_bps` from raw byte offsets in the cached curve account data.
    ///
    /// ConstantSpreadOracle: bytes 72..80 (after disc+pubkey+chain+max_age+bps+offset)
    /// InventorySkewOracle: bytes 120..128 (after disc+pubkey+chain+max_age+7*8+offset)
    fn read_score_factor_from_curve_data(&self) -> Option<u64> {
        let data = self.curve_account_data.as_ref()?;
        let range = match self.pool.curve_type() {
            CurveType::ConstantSpreadOracle => 72..80,
            CurveType::InventorySkewOracle => 120..128,
            _ => return Some(0),
        };
        if data.len() < range.end {
            return None;
        }
        Some(u64::from_le_bytes(data[range].try_into().ok()?))
    }

    /// Gets the token program for a given mint by checking which vault it corresponds to
    ///
    /// Returns the detected token program (SPL Token or Token-2022) for the vault.
    /// Falls back to SPL Token if the vault hasn't been updated yet.
    fn get_token_program(&self, mint: &Pubkey) -> Pubkey {
        if *mint == self.pool.token_a_mint {
            self.token_a_program
                .unwrap_or_else(anchor_spl::token::spl_token::id)
        } else {
            self.token_b_program
                .unwrap_or_else(anchor_spl::token::spl_token::id)
        }
    }

    /// Returns the curve type of this pool
    pub fn curve_type(&self) -> CurveType {
        self.pool.curve_type()
    }

    /// Returns whether the pool is in withdrawals-only mode
    pub fn is_withdrawals_only(&self) -> bool {
        self.pool.withdrawals_only != 0
    }

    /// Returns true if the cached oracle price is too stale to quote on.
    ///
    /// Checks each price index in the curve's price chain against
    /// `max_age_secs + ORACLE_STALENESS_OFFSET_SECS`. The offset absorbs cache
    /// lag and clock drift so we don't produce false negatives.
    ///
    /// Returns false (not stale) if no curve/oracle data is cached yet, or for
    /// any index where the timestamp can't be read.
    fn is_oracle_stale(&self) -> bool {
        if !self.oracle_staleness_check {
            return false;
        }
        let curve_data = match &self.curve_account_data {
            Some(d) => d,
            None => return false,
        };

        let (scope_pubkey, price_chain, max_age_secs) = match self.pool.curve_type() {
            CurveType::ConstantSpreadOracle => {
                let curve =
                    match ConstantSpreadOracleCurve::try_deserialize(&mut curve_data.as_slice()) {
                        Ok(c) => c,
                        Err(_) => return false,
                    };
                (
                    curve.scope_price_feed,
                    curve.price_chain,
                    curve.max_age_secs,
                )
            }
            CurveType::InventorySkewOracle => {
                let curve =
                    match InventorySkewOracleCurve::try_deserialize(&mut curve_data.as_slice()) {
                        Ok(c) => c,
                        Err(_) => return false,
                    };
                (
                    curve.scope_price_feed,
                    curve.price_chain,
                    curve.max_age_secs,
                )
            }
            _ => return false,
        };

        let scope_account = match self.scope_price_feeds.get(&scope_pubkey) {
            Some(a) => a,
            None => return false,
        };

        // Resolve the staleness offset: explicit override, or auto = min(max_age_secs)
        // across the active (non-sentinel) price chain entries.
        let offset: i64 = self.oracle_staleness_offset_secs.unwrap_or_else(|| {
            price_chain
                .iter()
                .zip(max_age_secs.iter())
                .take_while(|(&idx, _)| idx != u16::MAX)
                .map(|(_, &age)| age as i64)
                .min()
                .unwrap_or(0)
        });

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        for i in 0..4 {
            let idx = price_chain[i];
            if idx == u16::MAX {
                break;
            }
            let threshold = (max_age_secs[i] as i64).saturating_add(offset).max(0) as u64;
            if let Some(ts) = oracle::fetch_scope_price_timestamp(scope_account, idx) {
                if now.saturating_sub(ts) > threshold {
                    return true;
                }
            }
        }

        false
    }

    /// Gets the Scope price feed pubkey for oracle-based curves
    ///
    /// For ConstantSpreadOracle and InventorySkewOracle curves, this returns the
    /// Scope price feed account pubkey stored in the curve configuration.
    /// For other curve types, returns None.
    pub fn get_scope_price_feed(&self, accounts_map: &AccountMap) -> Option<Pubkey> {
        let curve_account = accounts_map.get(&self.pool.swap_curve)?;
        let data = curve_account.data.as_slice();
        match self.pool.curve_type() {
            CurveType::ConstantSpreadOracle => {
                let curve = ConstantSpreadOracleCurve::try_deserialize(&mut &*data).ok()?;
                Some(curve.scope_price_feed)
            }
            CurveType::InventorySkewOracle => {
                let curve = InventorySkewOracleCurve::try_deserialize(&mut &*data).ok()?;
                Some(curve.scope_price_feed)
            }
            _ => None,
        }
    }

    /// Gets a quote for oracle-based curves with explicit account passing
    ///
    /// Alternative to `quote()` that allows passing Scope account directly in accounts_map
    /// instead of requiring it to be cached via `update()`.
    ///
    /// `score` is the DFlow flow score (0-4) for spread widening.
    /// Higher scores indicate more toxic flow and result in wider spreads.
    pub fn quote_oracle(
        &self,
        quote_params: &QuoteParams,
        accounts_map: &AccountMap,
        score: u8,
    ) -> Result<Quote> {
        if score > MAX_SCORE {
            anyhow::bail!("Invalid score: {} (max: {})", score, MAX_SCORE);
        }

        // Validate vaults are updated
        let (token_a_amount, token_b_amount) = match (&self.token_a_vault, &self.token_b_vault) {
            (Some(token_a_vault), Some(token_b_vault)) => {
                (token_a_vault.amount, token_b_vault.amount)
            }
            _ => anyhow::bail!("Token vaults not updated. Call update() first."),
        };

        // Determine trade direction and amounts
        let (trade_direction, source_vault_amount) =
            if quote_params.input_mint == self.pool.token_a_mint {
                (TradeDirection::AtoB, token_a_amount)
            } else if quote_params.input_mint == self.pool.token_b_mint {
                (TradeDirection::BtoA, token_b_amount)
            } else {
                anyhow::bail!(
                    "Invalid mint: {}. Expected one of pool mints (A: {}, B: {})",
                    quote_params.input_mint,
                    self.pool.token_a_mint,
                    self.pool.token_b_mint
                );
            };

        // Get curve account
        let curve_account = accounts_map
            .get(&self.pool.swap_curve)
            .ok_or_else(|| anyhow::anyhow!("Curve account not found: {}", self.pool.swap_curve))?;

        // Determine destination vault amount
        let destination_vault_amount = if quote_params.input_mint == self.pool.token_a_mint {
            token_b_amount
        } else {
            token_a_amount
        };

        // Calculate quote based on curve type
        let (in_amount, out_amount) = match self.pool.curve_type() {
            CurveType::ConstantSpreadOracle => {
                // Extract scope_price_feed from curve data
                let scope_price_feed = solana_sdk::pubkey::Pubkey::try_from(
                    &curve_account.data[8..40],
                )
                .map_err(|_| anyhow::anyhow!("Failed to parse scope_price_feed from curve data"))?;

                // Get Scope price feed account
                let scope_account = accounts_map.get(&scope_price_feed).ok_or_else(|| {
                    anyhow::anyhow!("Scope price feed account not found: {}", scope_price_feed)
                })?;

                if self.vault_capacity_target_bps == 0 {
                    // No cap: quote directly, binary search on overflow
                    match oracle::calculate_constant_spread_quote_with_score(
                        &self.pool.fees,
                        quote_params.amount,
                        destination_vault_amount,
                        trade_direction,
                        &curve_account.data,
                        scope_account,
                        self.token_a_decimals,
                        self.token_b_decimals,
                        self.score_multiplier_bps(score),
                    ) {
                        Ok((_src, dest, _fees)) => (quote_params.amount, dest),
                        Err(crate::error::SdkError::InsufficientLiquidity {
                            required,
                            available,
                        }) => search_max_input(quote_params.amount, required, available, |amt| {
                            match oracle::calculate_constant_spread_quote_with_score(
                                &self.pool.fees,
                                amt,
                                destination_vault_amount,
                                trade_direction,
                                &curve_account.data,
                                scope_account,
                                self.token_a_decimals,
                                self.token_b_decimals,
                                self.score_multiplier_bps(score),
                            ) {
                                Ok((_src, dest, _fees)) => SwapFit::Fits(dest),
                                Err(crate::error::SdkError::InsufficientLiquidity { .. }) => {
                                    SwapFit::ExceedsVault
                                }
                                Err(_) => SwapFit::OtherError,
                            }
                        }),
                        Err(e) => return Err(e.into()),
                    }
                } else {
                    // Preemptive estimation with configurable target
                    // Extract price_chain and price_offset_bps for estimation
                    // Layout: discriminator(8) + scope_price_feed(32) + price_chain([u16;4]=8) + base_spread_bps(8) + price_offset_bps(8)
                    if curve_account.data.len() < 48 {
                        anyhow::bail!(
                            "ConstantSpreadOracle curve account data too short: {} bytes (expected >= 48)",
                            curve_account.data.len()
                        );
                    }
                    let price_chain: [u16; 4] = [
                        u16::from_le_bytes([curve_account.data[40], curve_account.data[41]]),
                        u16::from_le_bytes([curve_account.data[42], curve_account.data[43]]),
                        u16::from_le_bytes([curve_account.data[44], curve_account.data[45]]),
                        u16::from_le_bytes([curve_account.data[46], curve_account.data[47]]),
                    ];

                    if curve_account.data.len() < 64 {
                        anyhow::bail!(
                            "ConstantSpreadOracle curve account data too short for price_offset_bps: {} bytes (expected >= 64)",
                            curve_account.data.len()
                        );
                    }
                    let price_offset_bps = i64::from_le_bytes([
                        curve_account.data[56],
                        curve_account.data[57],
                        curve_account.data[58],
                        curve_account.data[59],
                        curve_account.data[60],
                        curve_account.data[61],
                        curve_account.data[62],
                        curve_account.data[63],
                    ]);

                    // Fetch oracle price for estimation
                    let (price_value, price_exp) =
                        oracle::fetch_scope_price_chain(scope_account, &price_chain)?;

                    let total_fee_bps = self
                        .pool
                        .fees
                        .trade_fee_numerator
                        .saturating_add(self.pool.fees.owner_trade_fee_numerator)
                        .saturating_mul(10000)
                        .checked_div(self.pool.fees.trade_fee_denominator)
                        .ok_or_else(|| {
                            anyhow::anyhow!("Fee calculation failed: denominator is zero")
                        })?;

                    let estimated_input = estimate_max_input_for_vault(
                        quote_params.amount,
                        destination_vault_amount,
                        price_value,
                        price_exp,
                        price_offset_bps,
                        trade_direction,
                        total_fee_bps,
                        self.vault_capacity_target_bps,
                    );

                    match oracle::calculate_constant_spread_quote_with_score(
                        &self.pool.fees,
                        estimated_input,
                        destination_vault_amount,
                        trade_direction,
                        &curve_account.data,
                        scope_account,
                        self.token_a_decimals,
                        self.token_b_decimals,
                        self.score_multiplier_bps(score),
                    ) {
                        Ok((_src, dest, _fees)) => (estimated_input, dest),
                        Err(crate::error::SdkError::InsufficientLiquidity {
                            required,
                            available,
                        }) => {
                            let capped_input = cap_input_proportional(
                                estimated_input,
                                required,
                                available,
                                self.vault_capacity_target_bps,
                            );
                            match oracle::calculate_constant_spread_quote_with_score(
                                &self.pool.fees,
                                capped_input,
                                destination_vault_amount,
                                trade_direction,
                                &curve_account.data,
                                scope_account,
                                self.token_a_decimals,
                                self.token_b_decimals,
                                self.score_multiplier_bps(score),
                            ) {
                                Ok((_src, dest, _fees)) => (capped_input, dest),
                                Err(e) => return Err(e.into()),
                            }
                        }
                        Err(e) => return Err(e.into()),
                    }
                }
            }
            CurveType::InventorySkewOracle => {
                // Extract scope_price_feed from curve data
                let scope_price_feed = solana_sdk::pubkey::Pubkey::try_from(
                    &curve_account.data[8..40],
                )
                .map_err(|_| anyhow::anyhow!("Failed to parse scope_price_feed from curve data"))?;

                // Get Scope price feed account
                let scope_account = accounts_map.get(&scope_price_feed).ok_or_else(|| {
                    anyhow::anyhow!("Scope price feed account not found: {}", scope_price_feed)
                })?;

                if self.vault_capacity_target_bps == 0 {
                    // No cap: quote directly, binary search on overflow
                    match oracle::calculate_inventory_skew_quote_with_score(
                        &self.pool.fees,
                        quote_params.amount,
                        source_vault_amount,
                        destination_vault_amount,
                        trade_direction,
                        &curve_account.data,
                        scope_account,
                        self.token_a_decimals,
                        self.token_b_decimals,
                        self.score_multiplier_bps(score),
                    ) {
                        Ok((_src, dest, _fees)) => (quote_params.amount, dest),
                        Err(crate::error::SdkError::InsufficientLiquidity {
                            required,
                            available,
                        }) => search_max_input(quote_params.amount, required, available, |amt| {
                            match oracle::calculate_inventory_skew_quote_with_score(
                                &self.pool.fees,
                                amt,
                                source_vault_amount,
                                destination_vault_amount,
                                trade_direction,
                                &curve_account.data,
                                scope_account,
                                self.token_a_decimals,
                                self.token_b_decimals,
                                self.score_multiplier_bps(score),
                            ) {
                                Ok((_src, dest, _fees)) => SwapFit::Fits(dest),
                                Err(crate::error::SdkError::InsufficientLiquidity { .. }) => {
                                    SwapFit::ExceedsVault
                                }
                                Err(_) => SwapFit::OtherError,
                            }
                        }),
                        Err(e) => return Err(e.into()),
                    }
                } else {
                    // Preemptive estimation with configurable target
                    // Extract price_chain and price_offset_bps for estimation
                    if curve_account.data.len() < 48 {
                        anyhow::bail!(
                            "InventorySkewOracle curve account data too short: {} bytes (expected >= 48)",
                            curve_account.data.len()
                        );
                    }
                    let price_chain: [u16; 4] = [
                        u16::from_le_bytes([curve_account.data[40], curve_account.data[41]]),
                        u16::from_le_bytes([curve_account.data[42], curve_account.data[43]]),
                        u16::from_le_bytes([curve_account.data[44], curve_account.data[45]]),
                        u16::from_le_bytes([curve_account.data[46], curve_account.data[47]]),
                    ];

                    if curve_account.data.len() < 120 {
                        anyhow::bail!(
                            "InventorySkewOracle curve account data too short for price_offset_bps: {} bytes (expected >= 120)",
                            curve_account.data.len()
                        );
                    }
                    let price_offset_bps = i64::from_le_bytes([
                        curve_account.data[112],
                        curve_account.data[113],
                        curve_account.data[114],
                        curve_account.data[115],
                        curve_account.data[116],
                        curve_account.data[117],
                        curve_account.data[118],
                        curve_account.data[119],
                    ]);

                    let (price_value, price_exp) =
                        oracle::fetch_scope_price_chain(scope_account, &price_chain)?;

                    let total_fee_bps = self
                        .pool
                        .fees
                        .trade_fee_numerator
                        .saturating_add(self.pool.fees.owner_trade_fee_numerator)
                        .saturating_mul(10000)
                        .checked_div(self.pool.fees.trade_fee_denominator)
                        .ok_or_else(|| {
                            anyhow::anyhow!("Fee calculation failed: denominator is zero")
                        })?;

                    let estimated_input = estimate_max_input_for_vault(
                        quote_params.amount,
                        destination_vault_amount,
                        price_value,
                        price_exp,
                        price_offset_bps,
                        trade_direction,
                        total_fee_bps,
                        self.vault_capacity_target_bps,
                    );

                    match oracle::calculate_inventory_skew_quote_with_score(
                        &self.pool.fees,
                        estimated_input,
                        source_vault_amount,
                        destination_vault_amount,
                        trade_direction,
                        &curve_account.data,
                        scope_account,
                        self.token_a_decimals,
                        self.token_b_decimals,
                        self.score_multiplier_bps(score),
                    ) {
                        Ok((_src, dest, _fees)) => (estimated_input, dest),
                        Err(crate::error::SdkError::InsufficientLiquidity {
                            required,
                            available,
                        }) => {
                            let capped_input = cap_input_proportional(
                                estimated_input,
                                required,
                                available,
                                self.vault_capacity_target_bps,
                            );
                            match oracle::calculate_inventory_skew_quote_with_score(
                                &self.pool.fees,
                                capped_input,
                                source_vault_amount,
                                destination_vault_amount,
                                trade_direction,
                                &curve_account.data,
                                scope_account,
                                self.token_a_decimals,
                                self.token_b_decimals,
                                self.score_multiplier_bps(score),
                            ) {
                                Ok((_src, dest, _fees)) => (capped_input, dest),
                                Err(e) => return Err(e.into()),
                            }
                        }
                        Err(e) => return Err(e.into()),
                    }
                }
            }
            _ => anyhow::bail!(
                "quote_oracle() only supports oracle curves. Pool curve type: {:?}",
                self.pool.curve_type()
            ),
        };

        Ok(Quote {
            in_amount,
            out_amount,
        })
    }

    /// Update account data only if it has changed (performance optimization)
    pub fn update_if_changed(&mut self, accounts_map: &AccountMap) -> Result<bool> {
        let mut has_changes = false;

        // Helper to compute hash of account data
        let compute_hash = |account: &solana_sdk::account::Account| -> u64 {
            let mut hasher = DefaultHasher::new();
            account.data.hash(&mut hasher);
            hasher.finish()
        };

        // Check token A vault
        if let Some(account) = accounts_map.get(&self.pool.token_a_vault) {
            let new_hash = compute_hash(account);
            let cached_hash = self.account_hashes.get(&self.pool.token_a_vault);

            if cached_hash != Some(&new_hash) {
                has_changes = true;
                self.account_hashes
                    .insert(self.pool.token_a_vault, new_hash);

                // Detect token program from account owner
                self.token_a_program = Some(account.owner);

                let mut data = &account.data[..];
                self.token_a_vault = Some(TokenAccount::try_deserialize(&mut data)?);
            }
        }

        // Check token B vault
        if let Some(account) = accounts_map.get(&self.pool.token_b_vault) {
            let new_hash = compute_hash(account);
            let cached_hash = self.account_hashes.get(&self.pool.token_b_vault);

            if cached_hash != Some(&new_hash) {
                has_changes = true;
                self.account_hashes
                    .insert(self.pool.token_b_vault, new_hash);

                // Detect token program from account owner
                self.token_b_program = Some(account.owner);

                let mut data = &account.data[..];
                self.token_b_vault = Some(TokenAccount::try_deserialize(&mut data)?);
            }
        }

        // Update token decimals from mint accounts (decimals never change, no hash check needed)
        if let Some(mint_account) = accounts_map.get(&self.pool.token_a_mint) {
            if mint_account.data.len() > 44 {
                self.token_a_decimals = mint_account.data[44];
            }
        }
        if let Some(mint_account) = accounts_map.get(&self.pool.token_b_mint) {
            if mint_account.data.len() > 44 {
                self.token_b_decimals = mint_account.data[44];
            }
        }

        // Check curve account
        if let Some(curve_account) = accounts_map.get(&self.pool.swap_curve) {
            let new_hash = compute_hash(curve_account);
            let cached_hash = self.account_hashes.get(&self.pool.swap_curve);

            if cached_hash != Some(&new_hash) {
                has_changes = true;
                self.account_hashes.insert(self.pool.swap_curve, new_hash);
                self.curve_account_data = Some(curve_account.data.clone());
            }
        }

        Ok(has_changes)
    }

    /// Quote with a DFlow flow score for spread widening.
    ///
    /// `score` is 0-4 where higher scores indicate more toxic flow and result in wider spreads.
    /// This is the primary method DFlow calls. The trait `quote()` delegates here with score=0.
    pub fn quote_with_score(&self, quote_params: &QuoteParams, score: u8) -> Result<Quote> {
        if score > MAX_SCORE {
            anyhow::bail!("Invalid score: {} (max: {})", score, MAX_SCORE);
        }

        // For oracle curves, use cached Scope account from update()
        match self.pool.curve_type() {
            CurveType::ConstantSpreadOracle | CurveType::InventorySkewOracle => {
                // Extract scope pubkey from cached curve data
                let scope_pubkey = self.curve_account_data.as_ref().and_then(|data| {
                    if data.len() >= 40 {
                        Pubkey::try_from(&data[8..40]).ok()
                    } else {
                        None
                    }
                });

                if let (Some(curve_data), Some(_scope_pk), Some(scope_account)) = (
                    &self.curve_account_data,
                    scope_pubkey,
                    scope_pubkey.and_then(|pk| self.scope_price_feeds.get(&pk)),
                ) {
                    // We have cached oracle data - use it (may be stale)
                    let (token_a_amount, token_b_amount) =
                        match (&self.token_a_vault, &self.token_b_vault) {
                            (Some(token_a_vault), Some(token_b_vault)) => {
                                (token_a_vault.amount, token_b_vault.amount)
                            }
                            _ => anyhow::bail!("Token vaults not updated. Call update() first."),
                        };

                    let (trade_direction, source_vault_amount, destination_vault_amount) =
                        if quote_params.input_mint == self.pool.token_a_mint {
                            (TradeDirection::AtoB, token_a_amount, token_b_amount)
                        } else if quote_params.input_mint == self.pool.token_b_mint {
                            (TradeDirection::BtoA, token_b_amount, token_a_amount)
                        } else {
                            anyhow::bail!(
                                "Invalid mint: {}. Expected one of pool mints (A: {}, B: {})",
                                quote_params.input_mint,
                                self.pool.token_a_mint,
                                self.pool.token_b_mint
                            );
                        };

                    let (in_amount, out_amount) = match self.pool.curve_type() {
                        CurveType::ConstantSpreadOracle => {
                            if self.vault_capacity_target_bps == 0 {
                                // No cap: quote directly, binary search on overflow
                                match oracle::calculate_constant_spread_quote_with_score(
                                    &self.pool.fees,
                                    quote_params.amount,
                                    destination_vault_amount,
                                    trade_direction,
                                    curve_data,
                                    scope_account,
                                    self.token_a_decimals,
                                    self.token_b_decimals,
                                    self.score_multiplier_bps(score),
                                ) {
                                    Ok((_src, dest, _fees)) => (quote_params.amount, dest),
                                    Err(crate::error::SdkError::InsufficientLiquidity {
                                        required,
                                        available,
                                    }) => search_max_input(
                                        quote_params.amount,
                                        required,
                                        available,
                                        |amt| {
                                            match oracle::calculate_constant_spread_quote_with_score(
                                                &self.pool.fees,
                                                amt,
                                                destination_vault_amount,
                                                trade_direction,
                                                curve_data,
                                                scope_account,
                                                self.token_a_decimals,
                                                self.token_b_decimals,
                                                self.score_multiplier_bps(score),
                                            ) {
                                                Ok((_src, dest, _fees)) => SwapFit::Fits(dest),
                                                Err(
                                                    crate::error::SdkError::InsufficientLiquidity {
                                                        ..
                                                    },
                                                ) => SwapFit::ExceedsVault,
                                                Err(_) => SwapFit::OtherError,
                                            }
                                        },
                                    ),
                                    Err(e) => return Err(e.into()),
                                }
                            } else {
                                // Preemptive estimation with configurable target
                                if curve_data.len() < 48 {
                                    anyhow::bail!(
                                        "ConstantSpreadOracle curve data too short: {} bytes (expected >= 48)",
                                        curve_data.len()
                                    );
                                }
                                let price_chain: [u16; 4] = [
                                    u16::from_le_bytes([curve_data[40], curve_data[41]]),
                                    u16::from_le_bytes([curve_data[42], curve_data[43]]),
                                    u16::from_le_bytes([curve_data[44], curve_data[45]]),
                                    u16::from_le_bytes([curve_data[46], curve_data[47]]),
                                ];

                                if curve_data.len() < 64 {
                                    anyhow::bail!(
                                        "ConstantSpreadOracle curve data too short for price_offset_bps: {} bytes (expected >= 64)",
                                        curve_data.len()
                                    );
                                }
                                let price_offset_bps = i64::from_le_bytes([
                                    curve_data[56],
                                    curve_data[57],
                                    curve_data[58],
                                    curve_data[59],
                                    curve_data[60],
                                    curve_data[61],
                                    curve_data[62],
                                    curve_data[63],
                                ]);

                                let (price_value, price_exp) =
                                    oracle::fetch_scope_price_chain(scope_account, &price_chain)?;

                                let total_fee_bps = self
                                    .pool
                                    .fees
                                    .trade_fee_numerator
                                    .saturating_add(self.pool.fees.owner_trade_fee_numerator)
                                    .saturating_mul(10000)
                                    .checked_div(self.pool.fees.trade_fee_denominator)
                                    .ok_or_else(|| {
                                        anyhow::anyhow!(
                                            "Fee calculation failed: denominator is zero"
                                        )
                                    })?;

                                let estimated_input = estimate_max_input_for_vault(
                                    quote_params.amount,
                                    destination_vault_amount,
                                    price_value,
                                    price_exp,
                                    price_offset_bps,
                                    trade_direction,
                                    total_fee_bps,
                                    self.vault_capacity_target_bps,
                                );

                                match oracle::calculate_constant_spread_quote_with_score(
                                    &self.pool.fees,
                                    estimated_input,
                                    destination_vault_amount,
                                    trade_direction,
                                    curve_data,
                                    scope_account,
                                    self.token_a_decimals,
                                    self.token_b_decimals,
                                    self.score_multiplier_bps(score),
                                ) {
                                    Ok((_src, dest, _fees)) => (estimated_input, dest),
                                    Err(crate::error::SdkError::InsufficientLiquidity {
                                        required,
                                        available,
                                    }) => {
                                        let capped_input = cap_input_proportional(
                                            estimated_input,
                                            required,
                                            available,
                                            self.vault_capacity_target_bps,
                                        );
                                        match oracle::calculate_constant_spread_quote_with_score(
                                            &self.pool.fees,
                                            capped_input,
                                            destination_vault_amount,
                                            trade_direction,
                                            curve_data,
                                            scope_account,
                                            self.token_a_decimals,
                                            self.token_b_decimals,
                                            self.score_multiplier_bps(score),
                                        ) {
                                            Ok((_src, dest, _fees)) => (capped_input, dest),
                                            Err(e) => return Err(e.into()),
                                        }
                                    }
                                    Err(e) => return Err(e.into()),
                                }
                            }
                        }
                        CurveType::InventorySkewOracle => {
                            if self.vault_capacity_target_bps == 0 {
                                // No cap: quote directly, binary search on overflow
                                match oracle::calculate_inventory_skew_quote_with_score(
                                    &self.pool.fees,
                                    quote_params.amount,
                                    source_vault_amount,
                                    destination_vault_amount,
                                    trade_direction,
                                    curve_data,
                                    scope_account,
                                    self.token_a_decimals,
                                    self.token_b_decimals,
                                    self.score_multiplier_bps(score),
                                ) {
                                    Ok((_src, dest, _fees)) => (quote_params.amount, dest),
                                    Err(crate::error::SdkError::InsufficientLiquidity {
                                        required,
                                        available,
                                    }) => search_max_input(
                                        quote_params.amount,
                                        required,
                                        available,
                                        |amt| {
                                            match oracle::calculate_inventory_skew_quote_with_score(
                                                &self.pool.fees,
                                                amt,
                                                source_vault_amount,
                                                destination_vault_amount,
                                                trade_direction,
                                                curve_data,
                                                scope_account,
                                                self.token_a_decimals,
                                                self.token_b_decimals,
                                                self.score_multiplier_bps(score),
                                            ) {
                                                Ok((_src, dest, _fees)) => SwapFit::Fits(dest),
                                                Err(
                                                    crate::error::SdkError::InsufficientLiquidity {
                                                        ..
                                                    },
                                                ) => SwapFit::ExceedsVault,
                                                Err(_) => SwapFit::OtherError,
                                            }
                                        },
                                    ),
                                    Err(e) => return Err(e.into()),
                                }
                            } else {
                                // Preemptive estimation with configurable target
                                if curve_data.len() < 48 {
                                    anyhow::bail!(
                                        "InventorySkewOracle curve data too short: {} bytes (expected >= 48)",
                                        curve_data.len()
                                    );
                                }
                                let price_chain: [u16; 4] = [
                                    u16::from_le_bytes([curve_data[40], curve_data[41]]),
                                    u16::from_le_bytes([curve_data[42], curve_data[43]]),
                                    u16::from_le_bytes([curve_data[44], curve_data[45]]),
                                    u16::from_le_bytes([curve_data[46], curve_data[47]]),
                                ];

                                if curve_data.len() < 120 {
                                    anyhow::bail!(
                                        "InventorySkewOracle curve data too short for price_offset_bps: {} bytes (expected >= 120)",
                                        curve_data.len()
                                    );
                                }
                                let price_offset_bps = i64::from_le_bytes([
                                    curve_data[112],
                                    curve_data[113],
                                    curve_data[114],
                                    curve_data[115],
                                    curve_data[116],
                                    curve_data[117],
                                    curve_data[118],
                                    curve_data[119],
                                ]);

                                let (price_value, price_exp) =
                                    oracle::fetch_scope_price_chain(scope_account, &price_chain)?;

                                let total_fee_bps = self
                                    .pool
                                    .fees
                                    .trade_fee_numerator
                                    .saturating_add(self.pool.fees.owner_trade_fee_numerator)
                                    .saturating_mul(10000)
                                    .checked_div(self.pool.fees.trade_fee_denominator)
                                    .ok_or_else(|| {
                                        anyhow::anyhow!(
                                            "Fee calculation failed: denominator is zero"
                                        )
                                    })?;

                                let estimated_input = estimate_max_input_for_vault(
                                    quote_params.amount,
                                    destination_vault_amount,
                                    price_value,
                                    price_exp,
                                    price_offset_bps,
                                    trade_direction,
                                    total_fee_bps,
                                    self.vault_capacity_target_bps,
                                );

                                match oracle::calculate_inventory_skew_quote_with_score(
                                    &self.pool.fees,
                                    estimated_input,
                                    source_vault_amount,
                                    destination_vault_amount,
                                    trade_direction,
                                    curve_data,
                                    scope_account,
                                    self.token_a_decimals,
                                    self.token_b_decimals,
                                    self.score_multiplier_bps(score),
                                ) {
                                    Ok((_src, dest, _fees)) => (estimated_input, dest),
                                    Err(crate::error::SdkError::InsufficientLiquidity {
                                        required,
                                        available,
                                    }) => {
                                        let capped_input = cap_input_proportional(
                                            estimated_input,
                                            required,
                                            available,
                                            self.vault_capacity_target_bps,
                                        );
                                        match oracle::calculate_inventory_skew_quote_with_score(
                                            &self.pool.fees,
                                            capped_input,
                                            source_vault_amount,
                                            destination_vault_amount,
                                            trade_direction,
                                            curve_data,
                                            scope_account,
                                            self.token_a_decimals,
                                            self.token_b_decimals,
                                            self.score_multiplier_bps(score),
                                        ) {
                                            Ok((_src, dest, _fees)) => (capped_input, dest),
                                            Err(e) => return Err(e.into()),
                                        }
                                    }
                                    Err(e) => return Err(e.into()),
                                }
                            }
                        }
                        _ => unreachable!(),
                    };

                    return Ok(Quote {
                        in_amount,
                        out_amount,
                    });
                } else {
                    anyhow::bail!(
                        "Oracle curve requires Scope price feed. Include Scope account when calling update(), \
                        or use quote_oracle() instead."
                    );
                }
            }
            _ => {}
        }

        // Standard quote for non-oracle curves using kdex_client::quote
        let actual_amount_in = quote_params.amount;

        let (token_a_amount, token_b_amount) = match (&self.token_a_vault, &self.token_b_vault) {
            (Some(token_a_vault), Some(token_b_vault)) => {
                (token_a_vault.amount, token_b_vault.amount)
            }
            _ => anyhow::bail!("Token vaults not updated. Call update() first."),
        };

        let (trade_direction, source_amount, destination_amount) =
            if quote_params.input_mint == self.pool.token_a_mint {
                (TradeDirection::AtoB, token_a_amount, token_b_amount)
            } else if quote_params.input_mint == self.pool.token_b_mint {
                (TradeDirection::BtoA, token_b_amount, token_a_amount)
            } else {
                anyhow::bail!(
                    "Invalid mint: {}. Expected one of pool mints (A: {}, B: {})",
                    quote_params.input_mint,
                    self.pool.token_a_mint,
                    self.pool.token_b_mint
                );
            };

        // Get the curve data
        let curve_data = self
            .curve_account_data
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Curve not updated. Call update() first."))?;

        // Calculate swap using kdex_client::quote
        if self.vault_capacity_target_bps == 0 {
            // No cap: quote directly, binary search on overflow
            match kdex_client::quote::calculate_quote(
                self.pool.curve_type(),
                &self.pool.fees,
                actual_amount_in,
                source_amount,
                destination_amount,
                trade_direction,
                curve_data,
            ) {
                Ok(result) => Ok(Quote {
                    in_amount: actual_amount_in,
                    out_amount: result.destination_amount_swapped,
                }),
                Err(kdex_client::quote::QuoteError::InsufficientLiquidity {
                    required,
                    available,
                }) => {
                    let (consumed, capped_out) =
                        search_max_input(actual_amount_in, required, available, |amt| {
                            match kdex_client::quote::calculate_quote(
                                self.pool.curve_type(),
                                &self.pool.fees,
                                amt,
                                source_amount,
                                destination_amount,
                                trade_direction,
                                curve_data,
                            ) {
                                Ok(result) => SwapFit::Fits(result.destination_amount_swapped),
                                Err(kdex_client::quote::QuoteError::InsufficientLiquidity {
                                    ..
                                }) => SwapFit::ExceedsVault,
                                Err(_) => SwapFit::OtherError,
                            }
                        });
                    Ok(Quote {
                        in_amount: consumed,
                        out_amount: capped_out,
                    })
                }
                Err(e) => Err(anyhow::anyhow!("Swap calculation failed: {}", e)),
            }
        } else {
            // Preemptive estimation with configurable target
            match kdex_client::quote::calculate_quote(
                self.pool.curve_type(),
                &self.pool.fees,
                actual_amount_in,
                source_amount,
                destination_amount,
                trade_direction,
                curve_data,
            ) {
                Ok(result) => {
                    // Apply vault capacity limit proactively
                    let max_output = (destination_amount as u128)
                        .saturating_mul(self.vault_capacity_target_bps as u128)
                        .checked_div(10000)
                        .unwrap_or(0) as u64;

                    if result.destination_amount_swapped > max_output {
                        let capped_input = cap_input_proportional(
                            actual_amount_in,
                            result.destination_amount_swapped,
                            destination_amount,
                            self.vault_capacity_target_bps,
                        );
                        match kdex_client::quote::calculate_quote(
                            self.pool.curve_type(),
                            &self.pool.fees,
                            capped_input,
                            source_amount,
                            destination_amount,
                            trade_direction,
                            curve_data,
                        ) {
                            Ok(result_capped) => Ok(Quote {
                                in_amount: capped_input,
                                out_amount: result_capped.destination_amount_swapped,
                            }),
                            Err(e) => Err(anyhow::anyhow!(
                                "Swap calculation failed after capping: {}",
                                e
                            )),
                        }
                    } else {
                        Ok(Quote {
                            in_amount: actual_amount_in,
                            out_amount: result.destination_amount_swapped,
                        })
                    }
                }
                Err(kdex_client::quote::QuoteError::InsufficientLiquidity {
                    required,
                    available,
                }) => {
                    let capped_input = cap_input_proportional(
                        actual_amount_in,
                        required,
                        available,
                        self.vault_capacity_target_bps,
                    );
                    match kdex_client::quote::calculate_quote(
                        self.pool.curve_type(),
                        &self.pool.fees,
                        capped_input,
                        source_amount,
                        destination_amount,
                        trade_direction,
                        curve_data,
                    ) {
                        Ok(result) => Ok(Quote {
                            in_amount: capped_input,
                            out_amount: result.destination_amount_swapped,
                        }),
                        Err(e) => Err(anyhow::anyhow!(
                            "Swap calculation failed after capping: {}",
                            e
                        )),
                    }
                }
                Err(e) => Err(anyhow::anyhow!("Swap calculation failed: {}", e)),
            }
        }
    }
}

impl Amm for KDEXAmm {
    fn label(&self) -> String {
        self.label.clone()
    }

    fn program_id(&self) -> Pubkey {
        self.program_id
    }

    fn key(&self) -> Pubkey {
        self.pool_key
    }

    fn get_reserve_mints(&self) -> Vec<Pubkey> {
        vec![self.pool.token_a_mint, self.pool.token_b_mint]
    }

    fn get_accounts_to_update(&self) -> Vec<Pubkey> {
        let mut accounts = vec![
            self.pool.token_a_vault,
            self.pool.token_b_vault,
            self.pool.swap_curve,
            self.pool.token_a_mint,
            self.pool.token_b_mint,
        ];

        // For oracle curves, include Scope price feed if we can extract it
        match self.pool.curve_type() {
            CurveType::ConstantSpreadOracle | CurveType::InventorySkewOracle => {
                // If we have curve data cached, we can extract the Scope address
                if let Some(curve_data) = &self.curve_account_data {
                    if curve_data.len() >= 40 {
                        if let Ok(scope_feed) = Pubkey::try_from(&curve_data[8..40]) {
                            accounts.push(scope_feed);
                        }
                    }
                }
            }
            _ => {}
        }

        accounts
    }

    fn from_keyed_account(keyed_account: &KeyedAccount, _amm_context: &AmmContext) -> Result<Self> {
        KDEXAmm::new_from_keyed_account(keyed_account)
    }

    fn update(&mut self, accounts_map: &AccountMap) -> Result<()> {
        // Update token vaults and detect token programs
        self.token_a_vault = if let Some(account) = accounts_map.get(&self.pool.token_a_vault) {
            // Detect token program from account owner
            self.token_a_program = Some(account.owner);

            let mut data = &account.data[..];
            Some(TokenAccount::try_deserialize(&mut data)?)
        } else {
            None
        };

        self.token_b_vault = if let Some(account) = accounts_map.get(&self.pool.token_b_vault) {
            // Detect token program from account owner
            self.token_b_program = Some(account.owner);

            let mut data = &account.data[..];
            Some(TokenAccount::try_deserialize(&mut data)?)
        } else {
            None
        };

        // Update token decimals from mint accounts
        // SPL Token Mint layout: mint_authority(36) + supply(8) + decimals(1)
        // Decimals byte is at offset 44 for both SPL Token and Token-2022
        if let Some(mint_account) = accounts_map.get(&self.pool.token_a_mint) {
            if mint_account.data.len() > 44 {
                self.token_a_decimals = mint_account.data[44];
            }
        }
        if let Some(mint_account) = accounts_map.get(&self.pool.token_b_mint) {
            if mint_account.data.len() > 44 {
                self.token_b_decimals = mint_account.data[44];
            }
        }

        // Update curve data
        if let Some(curve_account) = accounts_map.get(&self.pool.swap_curve) {
            // Cache curve account data for quotes
            self.curve_account_data = Some(curve_account.data.clone());

            // For oracle curves, cache the Scope price feed account if available
            match self.pool.curve_type() {
                CurveType::ConstantSpreadOracle | CurveType::InventorySkewOracle => {
                    if let Some(scope_pubkey) = self.get_scope_price_feed(accounts_map) {
                        if let Some(scope_account) = accounts_map.get(&scope_pubkey) {
                            self.scope_price_feeds
                                .insert(scope_pubkey, scope_account.clone());
                        }
                    }
                }
                _ => {}
            }
        }

        Ok(())
    }

    fn quote(&self, quote_params: &QuoteParams) -> Result<Quote> {
        self.quote_with_score(quote_params, 0)
    }

    fn get_swap_and_account_metas(&self, swap_params: &SwapParams) -> Result<SwapAndAccountMetas> {
        let SwapParams {
            destination_mint,
            source_mint,
            source_token_account,
            destination_token_account,
            token_transfer_authority,
            ..
        } = swap_params;

        // Determine vaults based on source mint
        let (source_vault, destination_vault, source_fees_vault) =
            if *source_mint == self.pool.token_a_mint {
                (
                    self.pool.token_a_vault,
                    self.pool.token_b_vault,
                    self.pool.token_a_fees_vault,
                )
            } else {
                (
                    self.pool.token_b_vault,
                    self.pool.token_a_vault,
                    self.pool.token_b_fees_vault,
                )
            };

        // Get token programs from cached values (detected during update()) or fall back to SPL Token
        let source_token_program = self.get_token_program(source_mint);
        let destination_token_program = self.get_token_program(destination_mint);

        // For oracle curves, extract the scope_price_feed from cached curve data
        let scope_price_feed = match self.pool.curve_type() {
            CurveType::ConstantSpreadOracle | CurveType::InventorySkewOracle => {
                // Extract scope_price_feed from cached curve data
                // Layout: discriminator(8) + scope_price_feed(32) + ...
                if let Some(curve_data) = &self.curve_account_data {
                    if curve_data.len() >= 40 {
                        Pubkey::try_from(&curve_data[8..40]).map_err(|_| {
                            anyhow::anyhow!("Failed to parse scope_price_feed from curve data")
                        })?
                    } else {
                        anyhow::bail!("Curve data too short to contain scope_price_feed");
                    }
                } else {
                    anyhow::bail!(
                        "Oracle curve requires curve data to be cached. Call update() first."
                    );
                }
            }
            _ => self.program_id, // Use program_id as placeholder for non-oracle curves
        };

        // Build account metas for swap2
        let account_metas = build_swap2_account_metas(
            *token_transfer_authority,
            self.pool_key,
            self.pool.swap_curve,
            self.pool.pool_authority,
            *source_mint,
            *destination_mint,
            source_vault,
            destination_vault,
            source_fees_vault,
            *source_token_account,
            *destination_token_account,
            source_token_program,
            destination_token_program,
            self.program_id,
            scope_price_feed,
        );

        Ok(SwapAndAccountMetas {
            swap: Swap::Placeholder,
            account_metas,
        })
    }

    fn clone_amm(&self) -> Box<dyn Amm + Send + Sync> {
        Box::new(self.clone())
    }

    fn has_dynamic_accounts(&self) -> bool {
        false
    }

    fn supports_exact_out(&self) -> bool {
        false
    }

    fn is_active(&self) -> bool {
        if self.is_withdrawals_only() {
            return false;
        }
        if matches!(
            self.pool.curve_type(),
            CurveType::ConstantSpreadOracle | CurveType::InventorySkewOracle
        ) && self.is_oracle_stale()
        {
            return false;
        }
        true
    }
}

/// Build the account metas for a KDEX swap2 instruction.
///
/// Same layout as the regular swap but without the instructions sysvar
/// and without score_signer (DFlow adds that on their side).
#[allow(clippy::too_many_arguments)]
fn build_swap2_account_metas(
    token_transfer_authority: Pubkey,
    pool_key: Pubkey,
    swap_curve: Pubkey,
    pool_authority: Pubkey,
    source_mint: Pubkey,
    destination_mint: Pubkey,
    source_vault: Pubkey,
    destination_vault: Pubkey,
    source_fees_vault: Pubkey,
    source_token_account: Pubkey,
    destination_token_account: Pubkey,
    source_token_program: Pubkey,
    destination_token_program: Pubkey,
    source_token_host_fees_account: Pubkey,
    scope_price_feed: Pubkey,
) -> Vec<solana_sdk::instruction::AccountMeta> {
    use solana_sdk::instruction::AccountMeta;
    vec![
        AccountMeta::new_readonly(token_transfer_authority, true),
        AccountMeta::new(pool_key, false),
        AccountMeta::new_readonly(swap_curve, false),
        AccountMeta::new_readonly(pool_authority, false),
        AccountMeta::new_readonly(source_mint, false),
        AccountMeta::new_readonly(destination_mint, false),
        AccountMeta::new(source_vault, false),
        AccountMeta::new(destination_vault, false),
        AccountMeta::new(source_fees_vault, false),
        AccountMeta::new(source_token_account, false),
        AccountMeta::new(destination_token_account, false),
        AccountMeta::new_readonly(source_token_program, false),
        AccountMeta::new_readonly(destination_token_program, false),
        AccountMeta::new(source_token_host_fees_account, false),
        AccountMeta::new_readonly(scope_price_feed, false),
    ]
}
