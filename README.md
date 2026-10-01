# Zenex Util Contracts

Soroban contracts used with the Zenex markets.

| Contract | |
|---|---|
| [fee-forwarder](fee-forwarder/README.md) | Collects a relay fee in a token, then makes the user's call. |
| [market-router](market-router/README.md) | Batches calls and runs the create-and-fill flows. |
| [session-policy](session-policy/README.md) | Smart-account policy that limits a session key to trading. |
| [referral](referral/README.md) | Records a referral as an event. |

## Build

Rust 1.98.1 (`rust-toolchain.toml`) and stellar-cli 27.1.0.

```
make        # build
make test
make fmt
```

OpenZeppelin `stellar-fee-abstraction` and `stellar-accounts` are pinned to commit `df602b6` of
their `v0.9.0` branch, which is unreleased and unaudited.

## Releases

A `vX.Y.Z` tag builds every contract with stellar.expert's
[soroban-build-workflow](https://github.com/stellar-expert/soroban-build-workflow) and attaches the
WASM to a GitHub release.

## License

AGPL-3.0
