//! Error types for the Hyperplane SDK
//!
//! This module provides custom error types that give SDK consumers better error handling
//! capabilities compared to generic anyhow errors.

use solana_sdk::pubkey::Pubkey;
use thiserror::Error;

/// Result type alias using SdkError
pub type Result<T> = std::result::Result<T, SdkError>;

/// Custom error type for the Hyperplane SDK
///
/// This enum provides specific error variants for different failure cases,
/// allowing consumers to pattern match and handle errors appropriately.
#[derive(Debug, Error)]
pub enum SdkError {
    /// Account deserialization failed
    #[error("Failed to deserialize account data: {0}")]
    DeserializationError(String),

    /// Token vaults have not been updated
    #[error("Token vaults not updated. Call update() with vault accounts before calling quote()")]
    VaultsNotUpdated,

    /// Swap curve has not been updated
    #[error("Swap curve not updated. Call update() with curve account before calling quote()")]
    CurveNotUpdated,

    /// Invalid mint provided in quote parameters
    #[error("Invalid mint: {mint}. Expected one of pool mints (A: {token_a}, B: {token_b})")]
    InvalidMint {
        mint: Pubkey,
        token_a: Pubkey,
        token_b: Pubkey,
    },

    /// Swap calculation failed
    #[error("Swap calculation failed: {0}")]
    SwapCalculationError(String),

    /// Invalid account owner
    #[error("Invalid account owner: expected {expected}, got {actual}")]
    InvalidAccountOwner { expected: Pubkey, actual: Pubkey },

    /// Account not found in account map
    #[error("Account {0} not found in provided accounts map")]
    AccountNotFound(Pubkey),

    /// Invalid pool state
    #[error("Invalid pool state: {0}")]
    InvalidPoolState(String),

    /// Oracle-specific errors
    #[error("Oracle error: {0}")]
    OracleError(String),

    /// Invalid oracle configuration
    #[error("Invalid oracle configuration: {0}")]
    InvalidOracleConfig(String),

    /// Curve calculation error
    #[error("Curve calculation error: {0}")]
    CurveError(String),

    /// RPC client error
    #[cfg(feature = "rpc-client")]
    #[error("RPC client error: {0}")]
    RpcClientError(#[from] Box<solana_client::client_error::ClientError>),

    /// Transaction error
    #[error("Transaction error: {0}")]
    TransactionError(String),

    /// Anchor error wrapper (used for curve calculations and deserialization)
    #[error("Anchor error: {0}")]
    AnchorError(#[from] anchor_lang::error::Error),

    /// Generic error for cases not covered by specific variants
    #[error("{0}")]
    Generic(String),
}

impl From<std::io::Error> for SdkError {
    fn from(err: std::io::Error) -> Self {
        SdkError::DeserializationError(err.to_string())
    }
}

impl From<anyhow::Error> for SdkError {
    fn from(err: anyhow::Error) -> Self {
        SdkError::Generic(err.to_string())
    }
}
