//! Hyperplane SDK
//!
//! This crate provides a Rust SDK for interacting with Hyperplane AMM pools on Solana.
//!
//! ## Features
//!
//! - **Pool Operations**: Initialize, swap, deposit, withdraw, and manage fees
//! - **Quote Calculation**: Off-chain swap quote calculation for all curve types
//! - **Pool Discovery**: Discover all Hyperplane pools on the network
//! - **Oracle Support**: Full support for oracle-based curves (ConstantSpreadOracle, InventorySkewOracle)
//!
//! ## Example
//!
//! ```ignore
//! use hyperplane_sdk::{QuoteCalculator, Quote};
//! use hyperplane::curve::calculator::TradeDirection;
//!
//! // Create a quote calculator
//! let calculator = QuoteCalculator::new(&rpc_client);
//!
//! // Get a quote for swapping
//! let quote = calculator.get_quote(
//!     pool_address,
//!     TradeDirection::AtoB,
//!     1_000_000,
//! ).await?;
//!
//! println!("Output: {} tokens", quote.out_amount);
//! println!("Fees: {} tokens", quote.total_fees);
//! ```

pub mod client;
pub mod config;
pub mod error;
pub mod oracle;
pub mod pool;
pub mod quote;

// Re-export commonly used types at crate root
pub use client::{Config, HyperplaneClient};
pub use config::PoolConfigValue;
pub use error::{Result, SdkError};
pub use pool::{discover_pools, filter_pool_accounts, PoolInfo, HYPERPLANE_PROGRAM_ID};
pub use quote::{Quote, QuoteCalculator};

// Re-export serde-gated types
#[cfg(feature = "serde")]
pub use config::InitializePoolConfig;

// Re-export commonly used types from hyperplane
pub use hyperplane::curve::calculator::TradeDirection;
pub use hyperplane::curve::fees::Fees;
pub use hyperplane::ix::{Initialize, UpdatePoolConfig};
pub use hyperplane::state::SwapPool;
pub use hyperplane::InitialSupply;

// Re-export orbit-link types for transaction building
pub use orbit_link::async_client::AsyncClient;
pub use orbit_link::OrbitLink;
