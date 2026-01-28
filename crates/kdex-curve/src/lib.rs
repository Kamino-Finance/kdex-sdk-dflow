//! KDEX Curve Math Library
//!
//! This crate provides the core mathematical functions for KDEX (Hyperplane) AMM pools.
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
pub mod math;
pub mod offset;
pub mod oracle;
pub mod stable;

// Re-export commonly used types
pub use error::CurveError;

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
