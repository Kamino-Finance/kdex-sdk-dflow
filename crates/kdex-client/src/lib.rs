//! KDEX Client
//!
//! Generated Rust client for the KDEX AMM program.
//!
//! This crate provides:
//! - Account types (SwapPool, etc.)
//! - Instruction builders (Swap, Deposit, Withdraw, etc.)
//! - CPI helpers for on-chain programs
//! - Optional fetch utilities for RPC clients
//! - Quote calculation for all curve types
//! - Optional oracle quote calculation (with "oracle" feature)

// Re-export generated code
#[allow(unused_imports, clippy::io_other_error)]
mod _generated;

pub use _generated::*;

// Re-export program ID at crate root
pub use _generated::programs::KDEX_ID;

// Convenience module aliases
pub mod generated {
    pub use crate::_generated::*;
}

// Curve state types for deserialization
pub mod curves;

// State types and traits
pub mod state;

// PDA utilities
pub mod pda;

// Swap instruction account meta builder
pub mod swap_ix;

// Quote calculation for non-oracle curves
pub mod quote;

// Local Scope oracle types (for oracle feature)
#[cfg(feature = "oracle")]
pub mod scope_types;

// Oracle quote calculation module (optional)
#[cfg(feature = "oracle")]
pub mod oracle;

// Re-export commonly used types at crate root
pub use pda::InitPoolPdas;
pub use state::{InitialSupply, SwapState};

// Re-export essential types from kdex-curve
pub use kdex_curve::{CurveType, Fees, TradeDirection};
