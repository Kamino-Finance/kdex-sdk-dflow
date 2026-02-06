//! State types and traits for KDEX pools
//!
//! This module provides types and traits for working with KDEX pool state.

use crate::generated::types::Fees;
use kdex_curve::CurveType;
use solana_pubkey::Pubkey;

/// Trait for accessing swap pool state
pub trait SwapState {
    /// Bump seed used to generate the program address / authority
    fn bump_seed(&self) -> u8;
    /// The pool authority PDA - authority of the pool vaults and pool token mint
    fn pool_authority(&self) -> &Pubkey;
    /// Address of token A liquidity account
    fn token_a_account(&self) -> &Pubkey;
    /// Address of token B liquidity account
    fn token_b_account(&self) -> &Pubkey;
    /// Address of pool token mint
    fn pool_mint(&self) -> &Pubkey;
    /// Address of token A mint
    fn token_a_mint(&self) -> &Pubkey;
    /// Address of token B mint
    fn token_b_mint(&self) -> &Pubkey;
    /// Fees associated with swap
    fn fees(&self) -> &Fees;
    /// Curve type
    fn curve_type(&self) -> CurveType;
}

impl SwapState for crate::generated::accounts::SwapPool {
    fn bump_seed(&self) -> u8 {
        u8::try_from(self.pool_authority_bump_seed).unwrap_or(0)
    }

    fn pool_authority(&self) -> &Pubkey {
        &self.pool_authority
    }

    fn token_a_account(&self) -> &Pubkey {
        &self.token_a_vault
    }

    fn token_b_account(&self) -> &Pubkey {
        &self.token_b_vault
    }

    fn pool_mint(&self) -> &Pubkey {
        &self.pool_token_mint
    }

    fn token_a_mint(&self) -> &Pubkey {
        &self.token_a_mint
    }

    fn token_b_mint(&self) -> &Pubkey {
        &self.token_b_mint
    }

    fn fees(&self) -> &Fees {
        &self.fees
    }

    fn curve_type(&self) -> CurveType {
        CurveType::from_u64(self.curve_type).unwrap_or(CurveType::ConstantProduct)
    }
}

/// Initial token supply for pool initialization
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InitialSupply {
    /// Initial amount of token A to deposit
    pub initial_supply_a: u64,
    /// Initial amount of token B to deposit
    pub initial_supply_b: u64,
}

impl InitialSupply {
    /// Create a new InitialSupply
    pub fn new(initial_supply_a: u64, initial_supply_b: u64) -> Self {
        Self {
            initial_supply_a,
            initial_supply_b,
        }
    }
}
