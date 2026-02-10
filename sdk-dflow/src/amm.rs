//! AMM implementation for KDEX using the DFlow AMM interface
//!
//! This module provides a DFlow-compatible AMM implementation for interacting with
//! KDEX pools, supporting all curve types including oracle-based curves.

use anchor_lang::AccountDeserialize;
use anyhow::Result;
use kdex_client::generated::accounts::SwapPool;
use kdex_client::state::SwapState;
use kdex_client::{CurveType, TradeDirection, KDEX_ID};
use solana_sdk::{instruction::AccountMeta, pubkey::Pubkey};
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use crate::oracle;

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
        })
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

    /// Gets the Scope price feed pubkey for oracle-based curves
    ///
    /// For ConstantSpreadOracle and InventorySkewOracle curves, this returns the
    /// Scope price feed account pubkey stored in the curve configuration.
    /// For other curve types, returns None.
    pub fn get_scope_price_feed(&self, accounts_map: &AccountMap) -> Option<Pubkey> {
        match self.pool.curve_type() {
            CurveType::ConstantSpreadOracle | CurveType::InventorySkewOracle => {
                let curve_account = accounts_map.get(&self.pool.swap_curve)?;

                // Extract scope_price_feed from curve data
                // Layout: discriminator(8) + scope_price_feed(32) + ...
                if curve_account.data.len() >= 40 {
                    Pubkey::try_from(&curve_account.data[8..40]).ok()
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Gets a quote for oracle-based curves with explicit account passing
    ///
    /// Alternative to `quote()` that allows passing Scope account directly in accounts_map
    /// instead of requiring it to be cached via `update()`.
    pub fn quote_oracle(
        &self,
        quote_params: &QuoteParams,
        accounts_map: &AccountMap,
    ) -> Result<Quote> {
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
        let (_in_amount, out_amount, _total_fees) = match self.pool.curve_type() {
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

                oracle::calculate_constant_spread_quote(
                    &self.pool.fees,
                    quote_params.amount,
                    destination_vault_amount,
                    trade_direction,
                    &curve_account.data,
                    scope_account,
                    self.token_a_decimals,
                    self.token_b_decimals,
                )?
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

                oracle::calculate_inventory_skew_quote(
                    &self.pool.fees,
                    quote_params.amount,
                    source_vault_amount,
                    destination_vault_amount,
                    trade_direction,
                    &curve_account.data,
                    scope_account,
                    self.token_a_decimals,
                    self.token_b_decimals,
                )?
            }
            _ => anyhow::bail!(
                "quote_oracle() only supports oracle curves. Pool curve type: {:?}",
                self.pool.curve_type()
            ),
        };

        // DFlow Quote only contains in_amount and out_amount
        Ok(Quote {
            in_amount: quote_params.amount,
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

                    let (_in_amount, out_amount, _total_fees) = match self.pool.curve_type() {
                        CurveType::ConstantSpreadOracle => oracle::calculate_constant_spread_quote(
                            &self.pool.fees,
                            quote_params.amount,
                            destination_vault_amount,
                            trade_direction,
                            curve_data,
                            scope_account,
                            self.token_a_decimals,
                            self.token_b_decimals,
                        )?,
                        CurveType::InventorySkewOracle => oracle::calculate_inventory_skew_quote(
                            &self.pool.fees,
                            quote_params.amount,
                            source_vault_amount,
                            destination_vault_amount,
                            trade_direction,
                            curve_data,
                            scope_account,
                            self.token_a_decimals,
                            self.token_b_decimals,
                        )?,
                        _ => unreachable!(),
                    };

                    // DFlow Quote only contains in_amount and out_amount
                    return Ok(Quote {
                        in_amount: quote_params.amount,
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
        let result = kdex_client::quote::calculate_quote(
            self.pool.curve_type(),
            &self.pool.fees,
            actual_amount_in,
            source_amount,
            destination_amount,
            trade_direction,
            curve_data,
        )
        .map_err(|e| anyhow::anyhow!("Swap calculation failed: {}", e))?;

        // DFlow Quote only contains in_amount and out_amount
        Ok(Quote {
            in_amount: actual_amount_in,
            out_amount: result.destination_amount_swapped,
        })
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

        // Build account metas according to KDEX's Swap instruction
        let account_metas = vec![
            AccountMeta::new_readonly(*token_transfer_authority, true),
            AccountMeta::new(self.pool_key, false),
            AccountMeta::new_readonly(self.pool.swap_curve, false),
            AccountMeta::new_readonly(self.pool.pool_authority, false),
            AccountMeta::new_readonly(*source_mint, false),
            AccountMeta::new_readonly(*destination_mint, false),
            AccountMeta::new(source_vault, false),
            AccountMeta::new(destination_vault, false),
            AccountMeta::new(source_fees_vault, false),
            AccountMeta::new(*source_token_account, false),
            AccountMeta::new(*destination_token_account, false),
            AccountMeta::new_readonly(source_token_program, false),
            AccountMeta::new_readonly(destination_token_program, false),
            // source_token_host_fees_account - passing program_id means None
            AccountMeta::new(self.program_id, false),
            // scope_price_feed - real address for oracle curves, program_id (None) for others
            AccountMeta::new_readonly(scope_price_feed, false),
        ];

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
}
