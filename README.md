# kdex-sdk

Rust SDK for integrating with the KDEX AMM on Solana.

## Overview

This SDK provides the core libraries and integration adapters for building applications that interact with the KDEX AMM. It is extracted from the main [KDEX repository](https://github.com/Kamino-Finance/kdex-private) for standalone use.

## Project Structure

```
kdex-sdk/
├── Cargo.toml              # Workspace configuration
├── crates/
│   ├── kdex-curve/         # Curve math library
│   └── kdex-client/        # Generated client bindings
├── sdk/                    # Core SDK
└── sdk-dflow/  # Integration adapter
```

## Crates

### kdex-curve

Low-level curve mathematics library implementing KDEX's pricing algorithms. Includes:

- Constant Product (x*y=k)
- Constant Price (fixed rate)
- Offset (asymmetric constant product)
- Stable (StableSwap invariant)
- Oracle-based curves with spread calculations

### kdex-client

Generated client bindings for interacting with the KDEX on-chain program.

### sdk

Core SDK providing high-level abstractions for:

- Pool state management
- Quote calculations
- Swap instruction building
- Account fetching and parsing

## Usage

Add the relevant crates to your `Cargo.toml`:

```toml
[dependencies]
kdex-sdk = { path = "sdk" }
kdex-curve = { path = "crates/kdex-curve" }
kdex-client = { path = "crates/kdex-client" }

# For dex integration
kdex-sdk-dflow = { path = "sdk-dflow" }
```

## Building

```bash
# Check the workspace builds
cargo check

# Run tests
cargo test

# Build release
cargo build --release
```

## Program IDs

| Environment | Program ID |
|-------------|------------|
| Production  | `kdexv89r17wFQN1MY3auCX7QgWFyshWAji2LsLRVUQU` |

## Supported Curves

| Curve | Oracle | Description |
|-------|--------|-------------|
| Constant Product | No | Classic x*y=k formula |
| Constant Price | No | Fixed exchange rate |
| Offset | No | Asymmetric constant product with virtual offset |
| Stable | No | Curve Finance StableSwap invariant |
| Constant Spread Oracle | Yes | Fixed spreads around oracle price |
| Inventory Skew Oracle | Yes | Dynamic spreads based on inventory and trade size |

## License

BUSL-1.1
