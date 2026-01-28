//! Oracle-based Curve Family
//!
//! This module contains oracle-based pricing curves that use external price feeds
//! for swap pricing. Each curve variant implements different spread strategies.
//!
//! ## Curve Variants
//!
//! - **ConstantSpread**: Fixed spread from oracle price
//! - **InventorySkew**: Dynamic spreads based on inventory imbalance and trade size

pub mod constant_spread;
pub mod inventory_skew;

pub use constant_spread::swap as constant_spread_swap;
pub use inventory_skew::{swap as inventory_skew_swap, InventorySkewParams};

/// Basis points denominator (10,000 = 100%)
pub const BPS_DENOMINATOR: u128 = 10_000;

/// Maximum basis points value (100%)
pub const MAX_BPS: u64 = 10_000;
