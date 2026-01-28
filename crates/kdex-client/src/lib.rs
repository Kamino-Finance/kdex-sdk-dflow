//! Hyperplane Client
//!
//! Generated Rust client for the Hyperplane AMM program.
//!
//! This crate provides:
//! - Account types (SwapPool, etc.)
//! - Instruction builders (Swap, Deposit, Withdraw, etc.)
//! - CPI helpers for on-chain programs
//! - Optional fetch utilities for RPC clients

// Re-export generated code
mod _generated;

pub use _generated::*;

// Re-export program ID at crate root
pub use _generated::programs::HYPERPLANE_ID;

// Convenience module aliases
pub mod generated {
    pub use crate::_generated::*;
}
