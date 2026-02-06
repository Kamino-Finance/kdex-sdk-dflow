//! Local Scope oracle types for KDEX SDK
//!
//! This module provides the minimal Scope oracle types needed for price fetching,
//! avoiding a dependency on the scope-types crate.

use bytemuck::{Pod, Zeroable};
use solana_pubkey::Pubkey;

/// Maximum number of price entries in a Scope OraclePrices account
pub const MAX_ENTRIES: usize = 512;

/// Scope program ID
/// This is the mainnet Scope program address
pub const SCOPE_PROGRAM_ID: Pubkey =
    solana_pubkey::pubkey!("HFn8GnPADiny6XqUoWE8uRPPxb29ikn4yTuPa9MF2fWJ");

/// Returns the Scope program ID
pub fn id() -> Pubkey {
    SCOPE_PROGRAM_ID
}

/// Price value from Scope oracle
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Pod, Zeroable)]
#[repr(C)]
pub struct Price {
    /// Price value (scaled integer)
    pub value: u64,
    /// Price exponent (decimal places)
    pub exp: u64,
}

/// Dated price entry from Scope oracle
/// This matches the on-chain layout of a single price entry in the OraclePrices account
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Pod, Zeroable)]
#[repr(C)]
pub struct DatedPrice {
    /// The price data
    pub price: Price,
    /// Last update slot
    pub last_updated_slot: u64,
    /// Unix timestamp
    pub unix_timestamp: u64,
    /// Reserved for future use
    pub _reserved: [u64; 2],
    /// Reserved for future use
    pub _reserved2: [u16; 3],
    /// Current index of the dated price
    pub index: u16,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::size_of;

    #[test]
    fn test_price_size() {
        // Price should be 16 bytes (2 x u64)
        assert_eq!(size_of::<Price>(), 16);
    }

    #[test]
    fn test_dated_price_size() {
        // DatedPrice should be 56 bytes
        // Price(16) + last_updated_slot(8) + unix_timestamp(8) + _reserved(16) + _reserved2(6) + index(2) = 56
        assert_eq!(size_of::<DatedPrice>(), 56);
    }

    #[test]
    fn test_scope_program_id() {
        // Verify the program ID is correct
        assert_eq!(
            id().to_string(),
            "HFn8GnPADiny6XqUoWE8uRPPxb29ikn4yTuPa9MF2fWJ"
        );
    }
}
