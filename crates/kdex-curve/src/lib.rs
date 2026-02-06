//! KDEX Curve Math Library
//!
//! This crate provides the core mathematical functions for KDEX (KDEX) AMM pools.
//! It is designed to be a shared dependency between the on-chain program and the SDK,
//! ensuring consistent swap calculations across both environments.
//!
//! ## Supported Curve Types
//!
//! | Curve Type | Description |
//! |------------|-------------|
//! | ConstantProduct | Standard x*y=k AMM |
//! | ConstantPrice | Fixed price trading |
//! | Stable | Optimized for pegged assets (like Curve) |
//! | Offset | Constant product with virtual offset |
//! | ConstantSpreadOracle | Oracle-based with fixed spread |
//! | InventorySkewOracle | Oracle-based with inventory-aware spreads |
//!
//! ## Usage
//!
//! ```rust
//! use kdex_curve::{constant_product, TradeDirection, SwapResult};
//!
//! // Calculate a constant product swap
//! let result = constant_product::swap(
//!     1_000_000, // source amount
//!     10_000_000, // pool source balance
//!     10_000_000, // pool destination balance
//! ).unwrap();
//!
//! println!("Output: {}", result.destination_amount_swapped);
//! ```

#![cfg_attr(not(feature = "std"), no_std)]

pub mod constant_price;
pub mod constant_product;
pub mod error;
pub mod fees;
pub mod math;
pub mod offset;
pub mod oracle;
pub mod stable;

// Re-export commonly used types
pub use error::CurveError;
pub use fees::Fees;

/// Result type for curve calculations
pub type Result<T> = core::result::Result<T, CurveError>;

/// Direction of trade
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TradeDirection {
    /// Trading token A for token B
    AtoB,
    /// Trading token B for token A
    BtoA,
}

/// Result of a swap calculation (without fees)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SwapResult {
    /// Amount of source token consumed
    pub source_amount_swapped: u128,
    /// Amount of destination token produced
    pub destination_amount_swapped: u128,
}

impl SwapResult {
    /// Create a new swap result
    pub fn new(source_amount_swapped: u128, destination_amount_swapped: u128) -> Self {
        Self {
            source_amount_swapped,
            destination_amount_swapped,
        }
    }
}

/// Rounding direction for pool token calculations
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoundDirection {
    /// Round down (floor)
    Floor,
    /// Round up (ceiling)
    Ceiling,
}

/// Result of pool token to trading token conversion
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TradingTokenResult {
    /// Amount of token A
    pub token_a_amount: u128,
    /// Amount of token B
    pub token_b_amount: u128,
}

/// Type of curve used for swap calculations
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u64)]
pub enum CurveType {
    /// Uniswap-style constant product curve, invariant = token_a_amount * token_b_amount
    ConstantProduct = 1,
    /// Flat line, always providing 1:1 from one token to another
    ConstantPrice = 2,
    /// Offset curve, like Uniswap, but the token B side has a faked offset
    Offset = 3,
    /// Stable curve, like constant product with less slippage around a fixed price
    Stable = 4,
    /// Curve with constant spread around an oracle price
    ConstantSpreadOracle = 5,
    /// Skewed curve with inventory-aware dynamic spreads
    InventorySkewOracle = 6,
}

impl CurveType {
    /// Convert from u64 to CurveType
    pub fn from_u64(value: u64) -> Option<Self> {
        match value {
            1 => Some(CurveType::ConstantProduct),
            2 => Some(CurveType::ConstantPrice),
            3 => Some(CurveType::Offset),
            4 => Some(CurveType::Stable),
            5 => Some(CurveType::ConstantSpreadOracle),
            6 => Some(CurveType::InventorySkewOracle),
            _ => None,
        }
    }

    /// Check if this is an oracle-based curve
    pub fn is_oracle_curve(&self) -> bool {
        matches!(
            self,
            CurveType::ConstantSpreadOracle | CurveType::InventorySkewOracle
        )
    }
}
