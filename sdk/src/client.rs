//! Hyperplane client for pool operations
//!
//! This module provides the `HyperplaneClient` for interacting with Hyperplane pools,
//! including pool initialization, swaps, deposits, withdrawals, and fee management.

use anchor_lang::prelude::Pubkey;
use anchor_lang::system_program::System;
use anchor_lang::Id;
use anchor_spl::token::TokenAccount;
use anyhow::Result;
use hyperplane::{
    curve::base::CurveType,
    ix::{Initialize, UpdatePoolConfig},
    state::{ConstantSpreadOracleCurve, InventorySkewOracleCurve, SwapPool, SwapState},
    utils::seeds::{pda, pda::InitPoolPdas},
    InitialSupply,
};
use orbit_link::{async_client::AsyncClient, OrbitLink};
use solana_sdk::{
    rent::Rent,
    signature::{Keypair, Signer},
    sysvar::SysvarId,
};

/// Configuration for the Hyperplane client
#[derive(PartialEq, Eq, Clone, Debug)]
pub struct Config {
    /// Hyperplane program id
    pub program_id: Pubkey,
    /// Send the transaction without actually executing it
    pub dry_run: bool,
    /// Encode the transaction in base58 and base64 and print it to stdout
    /// Instructions which require private key signer (e.g. zero-copy account allocations) will not executed immediately
    pub multisig: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            program_id: hyperplane::ID,
            dry_run: false,
            multisig: false,
        }
    }
}

/// Client for interacting with Hyperplane pools
pub struct HyperplaneClient<T: AsyncClient, S: Signer + Send + Sync> {
    /// The underlying OrbitLink client
    pub client: OrbitLink<T, S>,
    /// Client configuration
    pub config: Config,
}

impl<T, S> HyperplaneClient<T, S>
where
    T: AsyncClient,
    S: Signer + Send + Sync,
{
    /// Create a new Hyperplane client
    pub async fn new(client: OrbitLink<T, S>, config: Config) -> Result<Self> {
        Ok(Self { client, config })
    }

    /// Initialize a new Hyperplane pool
    ///
    /// # Arguments
    /// * `admin` - Pool admin public key
    /// * `token_a_mint` - Token A mint address
    /// * `token_b_mint` - Token B mint address
    /// * `admin_token_a_ata` - Admin's token A associated token account
    /// * `admin_token_b_ata` - Admin's token B associated token account
    /// * `initialize` - Pool initialization parameters (fees, curve, initial supply)
    ///
    /// # Returns
    /// The public key of the newly created pool
    #[allow(clippy::too_many_arguments)]
    pub async fn initialize_pool(
        &self,
        admin: Pubkey,
        token_a_mint: Pubkey,
        token_b_mint: Pubkey,
        admin_token_a_ata: Pubkey,
        admin_token_b_ata: Pubkey,
        Initialize {
            fees,
            curve_parameters,
            initial_supply:
                InitialSupply {
                    initial_supply_a,
                    initial_supply_b,
                },
        }: Initialize,
    ) -> Result<(
        Pubkey,
        orbit_link::tx_builder::TxBuilder<'_, T, S>,
        Vec<Keypair>,
    )> {
        let pool_kp = Keypair::new();
        let admin_pool_token_ata = Keypair::new();

        // Determine token programs from mints
        let token_a_token_program = self.determine_token_program(&token_a_mint).await?;
        let token_b_token_program = self.determine_token_program(&token_b_mint).await?;

        let InitPoolPdas {
            curve,
            authority,
            token_a_vault,
            token_b_vault,
            pool_token_mint,
            token_a_fees_vault,
            token_b_fees_vault,
        } = pda::init_pool_pdas_program_id(
            &self.config.program_id,
            &pool_kp.pubkey(),
            &token_a_mint,
            &token_b_mint,
        );

        let mut tx = self.client.tx_builder().add_ix(
            // Account for the swap pool, zero copy
            self.client
                .create_account_ix(&pool_kp.pubkey(), SwapPool::LEN, &self.config.program_id)
                .await?,
        );

        let pool_token_program = spl_token::id();

        let mut signers = vec![
            pool_kp.insecure_clone(),
            admin_pool_token_ata.insecure_clone(),
        ];

        if self.config.multisig {
            // Allocate space and assign to token program for the admin pool token account
            // This is required because multisig does not support additional signers
            // Cannot fully init the token account as the mint does not exist yet
            tx = tx.add_ix(
                self.client
                    .create_account_ix(
                        &admin_pool_token_ata.pubkey(),
                        TokenAccount::LEN,
                        &pool_token_program,
                    )
                    .await?,
            );
            // For multisig, we need to send these separately first
            signers = vec![];
        }

        let (event_authority, _bump) =
            Pubkey::find_program_address(&[b"__event_authority"], &self.config.program_id);

        tx = tx.add_anchor_ix(
            &self.config.program_id,
            hyperplane::accounts::InitializePool {
                admin,
                pool: pool_kp.pubkey(),
                swap_curve: curve,
                pool_authority: authority,
                token_a_mint,
                token_b_mint,
                token_a_vault,
                token_b_vault,
                pool_token_mint,
                token_a_fees_vault,
                token_b_fees_vault,
                admin_token_a_ata,
                admin_token_b_ata,
                admin_pool_token_ata: admin_pool_token_ata.pubkey(),
                system_program: System::id(),
                rent: Rent::id(),
                pool_token_program,
                token_a_token_program,
                token_b_token_program,
                event_authority,
                program: self.config.program_id,
            },
            hyperplane::instruction::InitializePool {
                initial_supply_a,
                initial_supply_b,
                fees,
                curve_parameters,
            },
        );

        Ok((pool_kp.pubkey(), tx, signers))
    }

    /// Update pool configuration
    ///
    /// # Arguments
    /// * `admin` - Pool admin public key
    /// * `pool` - Pool address
    /// * `update` - Configuration update parameters
    pub async fn update_pool_config(
        &self,
        admin: Pubkey,
        pool: Pubkey,
        update: UpdatePoolConfig,
    ) -> Result<orbit_link::tx_builder::TxBuilder<'_, T, S>> {
        let swap_pool: SwapPool = self.client.get_anchor_account(&pool).await?;
        let (event_authority, _bump) =
            Pubkey::find_program_address(&[b"__event_authority"], &self.config.program_id);
        let tx = self.client.tx_builder().add_anchor_ix(
            &self.config.program_id,
            hyperplane::accounts::UpdatePoolConfig {
                admin,
                pool,
                swap_curve: swap_pool.swap_curve,
                event_authority,
                program: self.config.program_id,
            },
            hyperplane::instruction::UpdatePoolConfig::from(update),
        );

        Ok(tx)
    }

    /// Execute a token swap
    ///
    /// # Arguments
    /// * `signer` - Transaction signer
    /// * `pool` - Pool address
    /// * `source_mint` - Source token mint
    /// * `destination_mint` - Destination token mint
    /// * `amount_in` - Amount of source tokens to swap
    /// * `minimum_amount_out` - Minimum acceptable output amount (slippage protection)
    /// * `source_token_host_fees_account` - Optional host fees account
    #[allow(clippy::too_many_arguments)]
    pub async fn swap(
        &self,
        signer: Pubkey,
        pool: Pubkey,
        source_mint: Pubkey,
        destination_mint: Pubkey,
        amount_in: u64,
        minimum_amount_out: u64,
        source_token_host_fees_account: Option<Pubkey>,
    ) -> Result<orbit_link::tx_builder::TxBuilder<'_, T, S>> {
        let swap_pool: SwapPool = self.client.get_anchor_account(&pool).await?;

        // Validate mints
        if source_mint == destination_mint {
            return Err(anyhow::anyhow!(
                "Source and destination mints must be different"
            ));
        }
        if !((source_mint == swap_pool.token_a_mint && destination_mint == swap_pool.token_b_mint)
            || (source_mint == swap_pool.token_b_mint
                && destination_mint == swap_pool.token_a_mint))
        {
            return Err(anyhow::anyhow!(
                "Source and destination mints must match the pool's token A ({}) and token B ({})",
                swap_pool.token_a_mint,
                swap_pool.token_b_mint
            ));
        }

        // Fetch scope_price_feed from the curve account if oracle curve
        let scope_price_feed = match swap_pool.curve_type() {
            CurveType::ConstantSpreadOracle => {
                let curve: ConstantSpreadOracleCurve = self
                    .client
                    .get_anchor_account(&swap_pool.swap_curve)
                    .await?;
                Some(curve.scope_price_feed)
            }
            CurveType::InventorySkewOracle => {
                let curve: InventorySkewOracleCurve = self
                    .client
                    .get_anchor_account(&swap_pool.swap_curve)
                    .await?;
                Some(curve.scope_price_feed)
            }
            _ => None,
        };

        // Get source and destination token accounts
        let source_user_ata =
            anchor_spl::associated_token::get_associated_token_address_with_program_id(
                &signer,
                &source_mint,
                &self.determine_token_program(&source_mint).await?,
            );
        let destination_user_ata =
            anchor_spl::associated_token::get_associated_token_address_with_program_id(
                &signer,
                &destination_mint,
                &self.determine_token_program(&destination_mint).await?,
            );

        // Validate source account has sufficient balance
        let source_account: TokenAccount = self
            .client
            .get_anchor_account(&source_user_ata)
            .await
            .map_err(|_| {
            anyhow::anyhow!(
                "Source token account {} does not exist. Create it first.",
                source_user_ata
            )
        })?;

        if source_account.amount < amount_in {
            return Err(anyhow::anyhow!(
                "Insufficient balance in source account {}. Has {} tokens, need {} tokens.",
                source_user_ata,
                source_account.amount,
                amount_in
            ));
        }

        // Determine which vaults and fee vaults to use
        let (source_vault, destination_vault, source_token_fees_vault) =
            if source_mint == swap_pool.token_a_mint {
                (
                    swap_pool.token_a_vault,
                    swap_pool.token_b_vault,
                    swap_pool.token_a_fees_vault,
                )
            } else {
                (
                    swap_pool.token_b_vault,
                    swap_pool.token_a_vault,
                    swap_pool.token_b_fees_vault,
                )
            };

        let source_token_program = self.determine_token_program(&source_mint).await?;
        let destination_token_program = self.determine_token_program(&destination_mint).await?;

        // Create destination ATA if it doesn't exist
        let tx = self.client.tx_builder().add_ix(
            spl_associated_token_account::instruction::create_associated_token_account_idempotent(
                &self.client.payer()?.pubkey(),
                &signer,
                &destination_mint,
                &destination_token_program,
            ),
        );

        let (event_authority, _bump) =
            Pubkey::find_program_address(&[b"__event_authority"], &self.config.program_id);

        let accounts = hyperplane::accounts::Swap {
            signer,
            pool,
            swap_curve: swap_pool.swap_curve,
            pool_authority: swap_pool.pool_authority,
            source_mint,
            destination_mint,
            source_vault,
            destination_vault,
            source_token_fees_vault,
            source_user_ata,
            destination_user_ata,
            source_token_program,
            destination_token_program,
            source_token_host_fees_account,
            scope_price_feed,
            event_authority,
            program: self.config.program_id,
        };

        let tx = tx.add_anchor_ix(
            &self.config.program_id,
            accounts,
            hyperplane::instruction::Swap {
                amount_in,
                minimum_amount_out,
            },
        );

        Ok(tx)
    }

    /// Deposit liquidity into a pool
    ///
    /// # Arguments
    /// * `signer` - Transaction signer
    /// * `pool` - Pool address
    /// * `pool_token_amount` - Amount of pool tokens to mint
    /// * `maximum_token_a_amount` - Maximum token A to deposit
    /// * `maximum_token_b_amount` - Maximum token B to deposit
    pub async fn deposit(
        &self,
        signer: Pubkey,
        pool: Pubkey,
        pool_token_amount: u64,
        maximum_token_a_amount: u64,
        maximum_token_b_amount: u64,
    ) -> Result<orbit_link::tx_builder::TxBuilder<'_, T, S>> {
        let swap_pool: SwapPool = self.client.get_anchor_account(&pool).await?;

        let token_a_token_program = self
            .determine_token_program(&swap_pool.token_a_mint)
            .await?;
        let token_b_token_program = self
            .determine_token_program(&swap_pool.token_b_mint)
            .await?;
        let pool_token_program = spl_token::id();

        let token_a_user_ata =
            anchor_spl::associated_token::get_associated_token_address_with_program_id(
                &signer,
                &swap_pool.token_a_mint,
                &token_a_token_program,
            );
        let token_b_user_ata =
            anchor_spl::associated_token::get_associated_token_address_with_program_id(
                &signer,
                &swap_pool.token_b_mint,
                &token_b_token_program,
            );
        let pool_token_user_ata =
            anchor_spl::associated_token::get_associated_token_address_with_program_id(
                &signer,
                &swap_pool.pool_token_mint,
                &pool_token_program,
            );

        let tx = self
            .client
            .tx_builder()
            .add_ix(
                spl_associated_token_account::instruction::create_associated_token_account_idempotent(
                    &self.client.payer()?.pubkey(),
                    &signer,
                    &swap_pool.pool_token_mint,
                    &pool_token_program,
                ),
            )
            .add_anchor_ix(
                &self.config.program_id,
                hyperplane::accounts::Deposit {
                    signer,
                    pool,
                    swap_curve: swap_pool.swap_curve,
                    pool_authority: swap_pool.pool_authority,
                    token_a_mint: swap_pool.token_a_mint,
                    token_b_mint: swap_pool.token_b_mint,
                    token_a_vault: swap_pool.token_a_vault,
                    token_b_vault: swap_pool.token_b_vault,
                    pool_token_mint: swap_pool.pool_token_mint,
                    token_a_user_ata,
                    token_b_user_ata,
                    pool_token_user_ata,
                    pool_token_program,
                    token_a_token_program,
                    token_b_token_program,
                    event_authority: {
                        let (ea, _) = Pubkey::find_program_address(
                            &[b"__event_authority"],
                            &self.config.program_id,
                        );
                        ea
                    },
                    program: self.config.program_id,
                },
                hyperplane::instruction::Deposit {
                    pool_token_amount,
                    maximum_token_a_amount,
                    maximum_token_b_amount,
                },
            );

        Ok(tx)
    }

    /// Withdraw liquidity from a pool
    ///
    /// # Arguments
    /// * `signer` - Transaction signer
    /// * `pool` - Pool address
    /// * `pool_token_amount` - Amount of pool tokens to burn
    /// * `minimum_token_a_amount` - Minimum token A to receive
    /// * `minimum_token_b_amount` - Minimum token B to receive
    pub async fn withdraw(
        &self,
        signer: Pubkey,
        pool: Pubkey,
        pool_token_amount: u64,
        minimum_token_a_amount: Option<u64>,
        minimum_token_b_amount: Option<u64>,
    ) -> Result<orbit_link::tx_builder::TxBuilder<'_, T, S>> {
        let swap_pool: SwapPool = self.client.get_anchor_account(&pool).await?;

        let token_a_token_program = self
            .determine_token_program(&swap_pool.token_a_mint)
            .await?;
        let token_b_token_program = self
            .determine_token_program(&swap_pool.token_b_mint)
            .await?;
        let pool_token_program = spl_token::id();

        let token_a_user_ata =
            anchor_spl::associated_token::get_associated_token_address_with_program_id(
                &signer,
                &swap_pool.token_a_mint,
                &token_a_token_program,
            );
        let token_b_user_ata =
            anchor_spl::associated_token::get_associated_token_address_with_program_id(
                &signer,
                &swap_pool.token_b_mint,
                &token_b_token_program,
            );
        let pool_token_user_ata =
            anchor_spl::associated_token::get_associated_token_address_with_program_id(
                &signer,
                &swap_pool.pool_token_mint,
                &pool_token_program,
            );

        let (event_authority, _bump) =
            Pubkey::find_program_address(&[b"__event_authority"], &self.config.program_id);

        let tx = self.client.tx_builder().add_anchor_ix(
            &self.config.program_id,
            hyperplane::accounts::Withdraw {
                signer,
                pool,
                swap_curve: swap_pool.swap_curve,
                pool_authority: swap_pool.pool_authority,
                token_a_mint: swap_pool.token_a_mint,
                token_b_mint: swap_pool.token_b_mint,
                token_a_vault: swap_pool.token_a_vault,
                token_b_vault: swap_pool.token_b_vault,
                pool_token_mint: swap_pool.pool_token_mint,
                token_a_fees_vault: swap_pool.token_a_fees_vault,
                token_b_fees_vault: swap_pool.token_b_fees_vault,
                token_a_user_ata,
                token_b_user_ata,
                pool_token_user_ata,
                pool_token_program,
                token_a_token_program,
                token_b_token_program,
                event_authority,
                program: self.config.program_id,
            },
            hyperplane::instruction::Withdraw {
                pool_token_amount,
                minimum_token_a_amount,
                minimum_token_b_amount,
            },
        );

        Ok(tx)
    }

    /// Withdraw accumulated fees from a pool
    ///
    /// # Arguments
    /// * `admin` - Pool admin public key
    /// * `pool` - Pool address
    /// * `fees_mint` - Mint of the fees to withdraw
    /// * `requested_withdraw_amount` - Amount to withdraw
    pub async fn withdraw_fees(
        &self,
        admin: Pubkey,
        pool: Pubkey,
        fees_mint: Pubkey,
        requested_withdraw_amount: u64,
    ) -> Result<orbit_link::tx_builder::TxBuilder<'_, T, S>> {
        let swap_pool: SwapPool = self.client.get_anchor_account(&pool).await?;

        let fees_token_program = self.determine_token_program(&fees_mint).await?;

        let fees_vault = if fees_mint == swap_pool.token_a_mint {
            swap_pool.token_a_fees_vault
        } else if fees_mint == swap_pool.token_b_mint {
            swap_pool.token_b_fees_vault
        } else {
            return Err(anyhow::anyhow!("Invalid fees mint"));
        };

        let admin_fees_ata =
            anchor_spl::associated_token::get_associated_token_address_with_program_id(
                &admin,
                &fees_mint,
                &fees_token_program,
            );

        let (event_authority, _bump) =
            Pubkey::find_program_address(&[b"__event_authority"], &self.config.program_id);

        let tx = self.client.tx_builder().add_anchor_ix(
            &self.config.program_id,
            hyperplane::accounts::WithdrawFees {
                admin,
                pool,
                pool_authority: swap_pool.pool_authority,
                fees_mint,
                fees_vault,
                admin_fees_ata,
                fees_token_program,
                event_authority,
                program: self.config.program_id,
            },
            hyperplane::instruction::WithdrawFees {
                requested_pool_token_amount: requested_withdraw_amount,
            },
        );

        Ok(tx)
    }

    pub async fn close_pool(
        &self,
        admin: Pubkey,
        pool: Pubkey,
    ) -> Result<orbit_link::tx_builder::TxBuilder<'_, T, S>> {
        let swap_pool: SwapPool = self.client.get_anchor_account(&pool).await?;

        let token_a_token_program = self
            .determine_token_program(&swap_pool.token_a_mint)
            .await?;
        let token_b_token_program = self
            .determine_token_program(&swap_pool.token_b_mint)
            .await?;
        let pool_token_program = spl_token::id();

        let (event_authority, _bump) =
            Pubkey::find_program_address(&[b"__event_authority"], &self.config.program_id);

        let tx = self.client.tx_builder().add_anchor_ix(
            &self.config.program_id,
            hyperplane::accounts::ClosePool {
                admin,
                pool,
                swap_curve: swap_pool.swap_curve,
                pool_authority: swap_pool.pool_authority,
                token_a_mint: swap_pool.token_a_mint,
                token_b_mint: swap_pool.token_b_mint,
                token_a_vault: swap_pool.token_a_vault,
                token_b_vault: swap_pool.token_b_vault,
                pool_token_mint: swap_pool.pool_token_mint,
                token_a_fees_vault: swap_pool.token_a_fees_vault,
                token_b_fees_vault: swap_pool.token_b_fees_vault,
                pool_token_program,
                token_a_token_program,
                token_b_token_program,
                event_authority,
                program: self.config.program_id,
            },
            hyperplane::instruction::ClosePool {},
        );

        Ok(tx)
    }

    /// Determine the token program for a given mint
    pub async fn determine_token_program(&self, mint: &Pubkey) -> Result<Pubkey> {
        let mint_account = self.client.client.get_account(mint).await?;
        Ok(mint_account.owner)
    }

    /// Get the underlying RPC client
    pub fn get_rpc(&self) -> &T {
        &self.client.client
    }
}
