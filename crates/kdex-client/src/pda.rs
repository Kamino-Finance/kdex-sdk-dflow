//! PDA (Program Derived Address) utilities for KDEX pools
//!
//! This module provides functions for deriving PDAs used by the KDEX program.

use solana_pubkey::Pubkey;

/// Seed prefix for pool authority PDA
pub const POOL_AUTHORITY_SEED: &[u8] = b"pool_authority";

/// Seed prefix for swap curve PDA
pub const SWAP_CURVE_SEED: &[u8] = b"swap_curve";

/// Derive the pool authority PDA
pub fn pool_authority(pool: &Pubkey, program_id: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[POOL_AUTHORITY_SEED, pool.as_ref()], program_id)
}

/// Derive the swap curve PDA
pub fn swap_curve(pool: &Pubkey, program_id: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[SWAP_CURVE_SEED, pool.as_ref()], program_id)
}

/// PDAs needed for pool initialization
#[derive(Clone, Debug)]
pub struct InitPoolPdas {
    /// Pool authority PDA
    pub pool_authority: Pubkey,
    /// Pool authority bump seed
    pub pool_authority_bump: u8,
    /// Swap curve PDA
    pub swap_curve: Pubkey,
    /// Swap curve bump seed
    pub swap_curve_bump: u8,
}

impl InitPoolPdas {
    /// Derive all PDAs needed for pool initialization
    pub fn find(pool: &Pubkey, program_id: &Pubkey) -> Self {
        let (pool_authority, pool_authority_bump) = pool_authority(pool, program_id);
        let (swap_curve, swap_curve_bump) = swap_curve(pool, program_id);
        Self {
            pool_authority,
            pool_authority_bump,
            swap_curve,
            swap_curve_bump,
        }
    }
}

/// Event authority PDA (for Anchor events)
pub fn event_authority(program_id: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[b"__event_authority"], program_id)
}
