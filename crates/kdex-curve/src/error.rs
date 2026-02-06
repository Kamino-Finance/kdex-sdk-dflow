//! Curve calculation errors

use thiserror::Error;

/// Errors that can occur during curve calculations
#[derive(Error, Debug, Clone, Copy, PartialEq, Eq)]
pub enum CurveError {
    /// Division by zero
    #[error("Division by zero")]
    DivisionByZero,

    /// Math overflow
    #[error("Math overflow")]
    Overflow,

    /// Zero amount provided
    #[error("Zero amount")]
    ZeroAmount,

    /// Calculation failure
    #[error("Calculation failure")]
    CalculationFailure,

    /// Conversion failure (e.g., U256 to u128)
    #[error("Conversion failure")]
    ConversionFailure,

    /// Invalid curve parameters
    #[error("Invalid curve parameters")]
    InvalidCurve,

    /// Zero trading tokens result
    #[error("Zero trading tokens")]
    ZeroTradingTokens,

    /// Invalid oracle configuration
    #[error("Invalid oracle configuration")]
    InvalidOracleConfig,

    /// Invalid fee configuration
    #[error("Invalid fee configuration")]
    InvalidFee,
}
