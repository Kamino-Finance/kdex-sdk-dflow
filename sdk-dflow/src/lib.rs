//! KDEX SDK for DFlow
//!
//! This crate provides a DFlow-compatible AMM interface for KDEX pools.

pub mod amm;
pub mod error;
pub mod oracle;

// Re-export commonly used types
pub use amm::{
    AccountMap, Amm, AmmContext, KDEXAmm, KeyedAccount, Quote, QuoteParams, Swap,
    SwapAndAccountMetas, SwapMode, SwapParams, TokenAccount,
};
pub use error::{Result, SdkError};
