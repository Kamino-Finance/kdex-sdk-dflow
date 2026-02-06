//! KDEX SDK
//!
//! This crate provides a Rust SDK for interacting with KDEX AMM pools on Solana.
//!
//! ## Features
//!
//! - **Quote Calculation**: Off-chain swap quote calculation for all curve types
//! - **Pool Discovery**: Discover all KDEX pools on the network
//! - **Oracle Support**: Full support for oracle-based curves (ConstantSpreadOracle, InventorySkewOracle)
//!
//! ## Example
//!
//! ```ignore
//! use kdex_sdk::{QuoteCalculator, Quote, TradeDirection};
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

pub mod error;
pub mod oracle;
pub mod pool;
pub mod quote;

// Re-export commonly used types at crate root
pub use error::{Result, SdkError};
pub use pool::{discover_pools, filter_pool_accounts, PoolInfo, KDEX_PROGRAM_ID};
pub use quote::{Quote, QuoteCalculator};

// Re-export commonly used types from kdex-client
pub use kdex_client::generated::accounts::SwapPool;
pub use kdex_client::generated::types::Fees;
pub use kdex_client::state::SwapState;
pub use kdex_client::{CurveType, TradeDirection, KDEX_ID};
