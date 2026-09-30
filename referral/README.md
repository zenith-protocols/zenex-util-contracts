# soroban-referral

Minimal Soroban contract for on-chain wallet-to-wallet referral attestation.

A wallet (`caller`) attests on chain that they were referred by another wallet (`referrer`). The contract verifies auth, blocks self-referral, and emits an `Attributed` event indexable by both addresses. No storage, no admin.

The event log is the canonical record. Indexers (Mercury, Goldsky, BigQuery) replay it to build the attribution graph.

## Function

```rust
attribute(caller: Address, referrer: Address) -> (Address, Address)
```

- Requires `caller`'s auth
- Errors with `ReferralError::SelfReferral` if `caller == referrer`
- Emits `Attributed { referee: caller, referrer }`
- Returns `(caller, referrer)` (useful for sim-based pre-flight)

## Event

```rust
Attributed {
    #[topic] referee: Address,
    #[topic] referrer: Address,
}
```

Topics: `("attributed", referee, referrer)`. Both addresses are indexable, so subscribers can filter on either side.

## Build, test, deploy

```bash
cargo test
stellar contract build --optimize

stellar contract deploy \
  --wasm target/wasm32v1-none/release/soroban_referral.wasm \
  --network testnet \
  --source-account <admin>
```

No constructor. Deployable as-is.
