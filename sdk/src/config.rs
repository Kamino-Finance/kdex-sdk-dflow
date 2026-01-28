//! Configuration types for Hyperplane pools
//!
//! This module provides configuration types for initializing and updating pools.

use hyperplane::state::{UpdatePoolConfigMode, UpdatePoolConfigValue};

#[cfg(feature = "serde")]
use hyperplane::{curve::fees::Fees, CurveUserParameters, InitialSupply};

/// Pool configuration update value
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PoolConfigValue {
    /// Set withdrawals-only mode
    WithdrawalsOnly(bool),
    /// Set price chain for oracle curves
    PriceChain([u16; 4]),
    /// Set max age in seconds for oracle prices
    MaxAgeSecs([u16; 4]),
}

impl From<PoolConfigValue> for hyperplane::instruction::UpdatePoolConfig {
    fn from(value: PoolConfigValue) -> Self {
        match value {
            PoolConfigValue::WithdrawalsOnly(val) => hyperplane::instruction::UpdatePoolConfig {
                mode: UpdatePoolConfigMode::WithdrawalsOnly as u16,
                value: UpdatePoolConfigValue::Bool(val).to_bytes(),
            },
            PoolConfigValue::PriceChain(chain) => hyperplane::instruction::UpdatePoolConfig {
                mode: UpdatePoolConfigMode::PriceChain as u16,
                value: UpdatePoolConfigValue::PriceChainArray(chain).to_bytes(),
            },
            PoolConfigValue::MaxAgeSecs(secs) => hyperplane::instruction::UpdatePoolConfig {
                mode: UpdatePoolConfigMode::MaxAgeSecs as u16,
                value: UpdatePoolConfigValue::MaxAgeSecs(secs).to_bytes(),
            },
        }
    }
}

impl From<PoolConfigValue> for hyperplane::ix::UpdatePoolConfig {
    fn from(value: PoolConfigValue) -> Self {
        match value {
            PoolConfigValue::WithdrawalsOnly(val) => hyperplane::ix::UpdatePoolConfig::new(
                UpdatePoolConfigMode::WithdrawalsOnly,
                UpdatePoolConfigValue::Bool(val),
            ),
            PoolConfigValue::PriceChain(chain) => hyperplane::ix::UpdatePoolConfig::new(
                UpdatePoolConfigMode::PriceChain,
                UpdatePoolConfigValue::PriceChainArray(chain),
            ),
            PoolConfigValue::MaxAgeSecs(secs) => hyperplane::ix::UpdatePoolConfig::new(
                UpdatePoolConfigMode::MaxAgeSecs,
                UpdatePoolConfigValue::MaxAgeSecs(secs),
            ),
        }
    }
}

/// Configuration for initializing a new pool
#[cfg(feature = "serde")]
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct InitializePoolConfig {
    /// Token A mint address
    pub token_a_mint: String,
    /// Token B mint address
    pub token_b_mint: String,
    /// Curve type and parameters
    pub curve: CurveUserParameters,
    /// Pool fees configuration
    pub fees: Fees,
    /// Initial token supply
    pub initial_supply: InitialSupply,
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    pub fn test_pool_config_withdrawals_only() {
        let config_val = PoolConfigValue::WithdrawalsOnly(true);
        let instruction_config: hyperplane::instruction::UpdatePoolConfig = config_val.into();
        assert_eq!(
            instruction_config.mode,
            UpdatePoolConfigMode::WithdrawalsOnly as u16
        );
    }

    #[test]
    pub fn test_pool_config_price_chain() {
        let config_val = PoolConfigValue::PriceChain([0, 65535, 65535, 65535]);
        let instruction_config: hyperplane::instruction::UpdatePoolConfig = config_val.into();
        assert_eq!(
            instruction_config.mode,
            UpdatePoolConfigMode::PriceChain as u16
        );
    }

    #[test]
    pub fn test_pool_config_max_age_secs() {
        let config_val = PoolConfigValue::MaxAgeSecs([65535, 0, 0, 0]);
        let instruction_config: hyperplane::instruction::UpdatePoolConfig = config_val.into();
        assert_eq!(
            instruction_config.mode,
            UpdatePoolConfigMode::MaxAgeSecs as u16
        );
    }
}
