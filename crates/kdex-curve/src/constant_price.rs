//! Constant Price Curve
//!
//! A simple fixed-price curve where one token trades at a constant rate
//! against the other. Token B is priced at `token_b_price` units of token A.

use crate::{
    math::{checked_div, checked_mul, checked_sub},
    CurveError, Result, SwapResult, TradeDirection,
};

/// Calculate a constant price swap
///
/// # Arguments
/// * `source_amount` - Amount of tokens being swapped in
/// * `token_b_price` - Price of token B in units of token A
/// * `trade_direction` - Direction of the trade (AtoB or BtoA)
///
/// # Returns
/// A `SwapResult` containing the amounts swapped
///
/// # Example
/// ```
/// use kdex_curve::{constant_price, TradeDirection};
///
/// // Token B costs 100 token A
/// let result = constant_price::swap(
///     1000,     // input 1000 token A
///     100,      // token B price is 100 A per B
///     TradeDirection::AtoB,
/// ).unwrap();
///
/// // Get 10 token B (1000 / 100)
/// assert_eq!(result.destination_amount_swapped, 10);
/// ```
pub fn swap(
    source_amount: u128,
    token_b_price: u64,
    trade_direction: TradeDirection,
) -> Result<SwapResult> {
    if source_amount == 0 {
        return Err(CurveError::ZeroAmount);
    }

    if token_b_price == 0 {
        return Err(CurveError::InvalidCurve);
    }

    let token_b_price = token_b_price as u128;

    let (source_amount_swapped, destination_amount_swapped) = match trade_direction {
        // Buying token B with token A: destination = source / price
        TradeDirection::AtoB => {
            let destination = checked_div(source_amount, token_b_price)?;

            // Floor the source amount to avoid taking too many tokens
            // Safe: token_b_price != 0 checked above
            let remainder = source_amount
                .checked_rem(token_b_price)
                .ok_or(CurveError::DivisionByZero)?;
            let source_consumed = if remainder > 0 {
                checked_sub(source_amount, remainder)?
            } else {
                source_amount
            };

            (source_consumed, destination)
        }
        // Selling token B for token A: destination = source * price
        TradeDirection::BtoA => {
            let destination = checked_mul(source_amount, token_b_price)?;
            (source_amount, destination)
        }
    };

    if source_amount_swapped == 0 || destination_amount_swapped == 0 {
        return Err(CurveError::ZeroTradingTokens);
    }

    Ok(SwapResult::new(
        source_amount_swapped,
        destination_amount_swapped,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_swap_a_to_b() {
        // Buy token B with 1000 A, price is 100 A per B
        let result = swap(1000, 100, TradeDirection::AtoB).unwrap();
        assert_eq!(result.source_amount_swapped, 1000);
        assert_eq!(result.destination_amount_swapped, 10);
    }

    #[test]
    fn test_swap_b_to_a() {
        // Sell 10 token B for A, price is 100 A per B
        let result = swap(10, 100, TradeDirection::BtoA).unwrap();
        assert_eq!(result.source_amount_swapped, 10);
        assert_eq!(result.destination_amount_swapped, 1000);
    }

    #[test]
    fn test_swap_a_to_b_with_remainder() {
        // Buy with 150 A, price is 100 A per B
        // Should consume 100 A, get 1 B, leave 50 A unconsumed
        let result = swap(150, 100, TradeDirection::AtoB).unwrap();
        assert_eq!(result.source_amount_swapped, 100);
        assert_eq!(result.destination_amount_swapped, 1);
    }

    #[test]
    fn test_swap_insufficient_amount() {
        // Try to buy with 50 A, but price is 100 A per B
        // Not enough to buy even 1 token B
        assert!(swap(50, 100, TradeDirection::AtoB).is_err());
    }

    #[test]
    fn test_swap_zero_amount() {
        assert!(swap(0, 100, TradeDirection::AtoB).is_err());
    }

    #[test]
    fn test_swap_zero_price() {
        assert!(swap(1000, 0, TradeDirection::AtoB).is_err());
    }

    #[test]
    fn test_swap_price_one() {
        // 1:1 swap
        let result = swap(100, 1, TradeDirection::AtoB).unwrap();
        assert_eq!(result.source_amount_swapped, 100);
        assert_eq!(result.destination_amount_swapped, 100);

        let result = swap(100, 1, TradeDirection::BtoA).unwrap();
        assert_eq!(result.source_amount_swapped, 100);
        assert_eq!(result.destination_amount_swapped, 100);
    }
}
