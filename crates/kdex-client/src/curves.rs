//! Curve state account types for KDEX pools
//!
//! These structs represent the on-chain curve account data and can be deserialized
//! from account data to access curve parameters for quote calculations.

use borsh::{BorshDeserialize, BorshSerialize};
use solana_pubkey::Pubkey;

/// Discriminator size for Anchor accounts
const DISCRIMINATOR_SIZE: usize = 8;

/// Constant price curve - always provides 1:1 from one token to another
#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, PartialEq, Default)]
pub struct ConstantPriceCurve {
    /// Discriminator (first 8 bytes of account data)
    pub discriminator: [u8; 8],
    /// Amount of token A required to get 1 token B
    pub token_b_price: u64,
    /// Padding to maintain account size
    pub _padding: [u64; 15],
}

impl ConstantPriceCurve {
    /// Account data length
    pub const LEN: usize = DISCRIMINATOR_SIZE + 128; // 8 + 8 + 15*8
}

/// Constant product curve - Uniswap-style x*y=k invariant
#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, PartialEq, Default)]
pub struct ConstantProductCurve {
    /// Discriminator (first 8 bytes of account data)
    pub discriminator: [u8; 8],
    /// Padding to maintain account size
    pub _padding: [u64; 16],
}

impl ConstantProductCurve {
    /// Account data length
    pub const LEN: usize = DISCRIMINATOR_SIZE + 128; // 8 + 16*8
}

/// Offset curve - constant product with virtual offset on token B
#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, PartialEq, Default)]
pub struct OffsetCurve {
    /// Discriminator (first 8 bytes of account data)
    pub discriminator: [u8; 8],
    /// Amount to offset the token B liquidity account
    pub token_b_offset: u64,
    /// Padding to maintain account size
    pub _padding: [u64; 15],
}

impl OffsetCurve {
    /// Account data length
    pub const LEN: usize = DISCRIMINATOR_SIZE + 128; // 8 + 8 + 15*8
}

/// Stable curve - optimized for pegged assets with reduced slippage
#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, PartialEq, Default)]
pub struct StableCurve {
    /// Discriminator (first 8 bytes of account data)
    pub discriminator: [u8; 8],
    /// Amplifier constant
    pub amp: u64,
    /// Amount of token A required to get 1 token B
    pub token_a_factor: u64,
    /// Amount of token B required to get 1 token A
    pub token_b_factor: u64,
    /// Padding to maintain account size
    pub _padding: [u64; 13],
}

impl StableCurve {
    /// Account data length
    pub const LEN: usize = DISCRIMINATOR_SIZE + 128; // 8 + 3*8 + 13*8
}

/// Constant spread oracle curve - oracle-based pricing with fixed spread
///
/// Uses a price chain mechanism:
/// - `price_chain`: Up to 4 Scope price indices that get multiplied together
/// - `max_age_secs`: Max staleness in seconds for each price index
///
/// Example: For an LST like bonkSOL, use price_chain=[387, 0] where:
/// - Index 387 = bonkSOL/SOL exchange rate
/// - Index 0 = SOL/USD price
///
/// Final price = bonkSOL/SOL * SOL/USD = bonkSOL/USD
#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, PartialEq, Default)]
pub struct ConstantSpreadOracleCurve {
    /// Discriminator (first 8 bytes of account data)
    pub discriminator: [u8; 8],
    /// Scope price feed account (OraclePrices account)
    pub scope_price_feed: Pubkey,
    /// Price chain - array of Scope price indices (up to 4)
    /// Values of u16::MAX (65535) terminate the chain
    pub price_chain: [u16; 4],
    /// Max age in seconds for each price index in the chain
    pub max_age_secs: [u16; 4],
    /// Spread in basis points from oracle price
    pub bps_from_oracle: u64,
    /// Price offset in basis points (can be negative)
    /// Applied as: adjusted_price = oracle_price * (10000 + price_offset_bps) / 10000
    pub price_offset_bps: i64,
    /// Padding to maintain account size
    pub _padding: [u64; 11],
}

impl ConstantSpreadOracleCurve {
    /// Account data length
    pub const LEN: usize = DISCRIMINATOR_SIZE + 152;
}

/// Inventory skew oracle curve - oracle-based with inventory-aware dynamic spreads
///
/// This curve adjusts bid/ask spreads based on:
/// - Current inventory position relative to equilibrium
/// - Trade size (larger trades = wider spreads)
/// - Inventory skew (push inventory back to balance)
#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, PartialEq, Default)]
pub struct InventorySkewOracleCurve {
    /// Discriminator (first 8 bytes of account data)
    pub discriminator: [u8; 8],
    /// Scope price feed account (OraclePrices account)
    pub scope_price_feed: Pubkey,
    /// Price chain - array of Scope price indices (up to 4)
    /// Values of u16::MAX (65535) terminate the chain
    pub price_chain: [u16; 4],
    /// Max age in seconds for each price index in the chain
    pub max_age_secs: [u16; 4],
    /// Base spread - minimum half-spread in basis points
    pub base_spread_bps: u64,
    /// Size spread coefficient - how much spread increases with trade size
    pub size_spread_bps: u64,
    /// Inventory skew adjustment - how much to widen spreads when off-balance
    pub skew_bps: u64,
    /// Target equilibrium inventory for token A (in native token units)
    pub inv_equilibrium: u64,
    /// Maximum inventory deviation reference (in native token units)
    pub inv_max: u64,
    /// Reference size for spread calculation (in native token units)
    pub q_ref: u64,
    /// Exponent for size impact, scaled by 10000 (e.g., 20000 = 2.0)
    pub alpha: u64,
    /// Price offset in basis points (can be negative)
    pub price_offset_bps: i64,
    /// Padding to maintain account size
    pub _padding: [u64; 5],
}

impl InventorySkewOracleCurve {
    /// Account data length
    pub const LEN: usize = DISCRIMINATOR_SIZE + 152;
}

// Implement anchor traits when anchor feature is enabled
#[cfg(feature = "anchor")]
mod anchor_impl {
    use super::*;

    impl anchor_lang::AccountDeserialize for ConstantPriceCurve {
        fn try_deserialize_unchecked(buf: &mut &[u8]) -> anchor_lang::Result<Self> {
            Ok(Self::deserialize(buf)?)
        }
    }
    impl anchor_lang::AccountSerialize for ConstantPriceCurve {}
    impl anchor_lang::Owner for ConstantPriceCurve {
        fn owner() -> Pubkey {
            crate::KDEX_ID
        }
    }

    impl anchor_lang::AccountDeserialize for ConstantProductCurve {
        fn try_deserialize_unchecked(buf: &mut &[u8]) -> anchor_lang::Result<Self> {
            Ok(Self::deserialize(buf)?)
        }
    }
    impl anchor_lang::AccountSerialize for ConstantProductCurve {}
    impl anchor_lang::Owner for ConstantProductCurve {
        fn owner() -> Pubkey {
            crate::KDEX_ID
        }
    }

    impl anchor_lang::AccountDeserialize for OffsetCurve {
        fn try_deserialize_unchecked(buf: &mut &[u8]) -> anchor_lang::Result<Self> {
            Ok(Self::deserialize(buf)?)
        }
    }
    impl anchor_lang::AccountSerialize for OffsetCurve {}
    impl anchor_lang::Owner for OffsetCurve {
        fn owner() -> Pubkey {
            crate::KDEX_ID
        }
    }

    impl anchor_lang::AccountDeserialize for StableCurve {
        fn try_deserialize_unchecked(buf: &mut &[u8]) -> anchor_lang::Result<Self> {
            Ok(Self::deserialize(buf)?)
        }
    }
    impl anchor_lang::AccountSerialize for StableCurve {}
    impl anchor_lang::Owner for StableCurve {
        fn owner() -> Pubkey {
            crate::KDEX_ID
        }
    }

    impl anchor_lang::AccountDeserialize for ConstantSpreadOracleCurve {
        fn try_deserialize_unchecked(buf: &mut &[u8]) -> anchor_lang::Result<Self> {
            Ok(Self::deserialize(buf)?)
        }
    }
    impl anchor_lang::AccountSerialize for ConstantSpreadOracleCurve {}
    impl anchor_lang::Owner for ConstantSpreadOracleCurve {
        fn owner() -> Pubkey {
            crate::KDEX_ID
        }
    }

    impl anchor_lang::AccountDeserialize for InventorySkewOracleCurve {
        fn try_deserialize_unchecked(buf: &mut &[u8]) -> anchor_lang::Result<Self> {
            Ok(Self::deserialize(buf)?)
        }
    }
    impl anchor_lang::AccountSerialize for InventorySkewOracleCurve {}
    impl anchor_lang::Owner for InventorySkewOracleCurve {
        fn owner() -> Pubkey {
            crate::KDEX_ID
        }
    }
}
