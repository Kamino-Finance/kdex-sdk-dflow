//! Pool discovery for KDEX
//!
//! This module provides functionality to discover and list KDEX pools.

use solana_sdk::hash::hash;
use solana_sdk::pubkey::Pubkey;

/// Default KDEX program ID
pub const KDEX_PROGRAM_ID: &str = "kdexv89r17wFQN1MY3auCX7QgWFyshWAji2LsLRVUQU";

/// Returns the 8-byte discriminator for SwapPool accounts
/// Computed as: hash("account:SwapPool")[..8]
pub fn swap_pool_discriminator() -> [u8; 8] {
    let discriminator_preimage = format!("account:{}", "SwapPool");
    let hash_result = hash(discriminator_preimage.as_bytes());
    let mut discriminator = [0u8; 8];
    discriminator.copy_from_slice(&hash_result.to_bytes()[..8]);
    discriminator
}

/// Information about a discovered pool
#[derive(Debug, Clone)]
pub struct PoolInfo {
    /// Pool public key
    pub pubkey: Pubkey,
    /// Account data size
    pub data_len: usize,
    /// Account owner (program ID)
    pub owner: Pubkey,
    /// Account lamports
    pub lamports: u64,
}

/// Discover all KDEX pools from a list of program accounts
///
/// # Arguments
/// * `accounts` - List of (pubkey, account) tuples from `get_program_accounts`
///
/// # Returns
/// A vector of `PoolInfo` for all discovered SwapPool accounts
pub fn filter_pool_accounts(accounts: &[(Pubkey, solana_sdk::account::Account)]) -> Vec<PoolInfo> {
    let discriminator = swap_pool_discriminator();

    accounts
        .iter()
        .filter(|(_, account)| account.data.len() >= 8 && account.data[0..8] == discriminator)
        .map(|(pubkey, account)| PoolInfo {
            pubkey: *pubkey,
            data_len: account.data.len(),
            owner: account.owner,
            lamports: account.lamports,
        })
        .collect()
}

/// Discover all KDEX pools using an RPC client
///
/// # Arguments
/// * `client` - RPC client
/// * `program_id` - KDEX program ID (use `kdex::ID` or custom)
///
/// # Returns
/// A vector of `PoolInfo` for all discovered SwapPool accounts
#[cfg(feature = "rpc-client")]
pub fn discover_pools(
    client: &solana_client::rpc_client::RpcClient,
    program_id: &Pubkey,
) -> anyhow::Result<Vec<PoolInfo>> {
    let accounts = client.get_program_accounts(program_id)?;
    Ok(filter_pool_accounts(&accounts))
}

/// Discover all KDEX pools using an async RPC client
///
/// # Arguments
/// * `client` - Async RPC client
/// * `program_id` - KDEX program ID (use `kdex::ID` or custom)
///
/// # Returns
/// A vector of `PoolInfo` for all discovered SwapPool accounts
#[cfg(feature = "rpc-client")]
pub async fn discover_pools_async(
    client: &solana_client::nonblocking::rpc_client::RpcClient,
    program_id: &Pubkey,
) -> anyhow::Result<Vec<PoolInfo>> {
    let accounts = client.get_program_accounts(program_id).await?;
    Ok(filter_pool_accounts(&accounts))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_swap_pool_discriminator() {
        let disc = swap_pool_discriminator();
        // The discriminator should be 8 bytes
        assert_eq!(disc.len(), 8);
        // It should be deterministic
        assert_eq!(disc, swap_pool_discriminator());
    }
}
