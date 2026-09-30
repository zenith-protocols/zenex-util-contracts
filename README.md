# Zenex Util Contracts

Small standalone contracts for Zenex, a perpetual futures exchange on
[Stellar](https://stellar.org) (Soroban). They sit outside the core protocol in
[zenex-contracts](https://github.com/zenith-protocols/zenex-contracts) and do not
depend on its interfaces.

## Contracts

| Crate | Description |
|-------|-------------|
| `referral` | Referral attestation: a wallet attests which wallet referred it |
| `session-policy` | Smart-account policy for trading session keys: a contract allowlist and a spend limit on the collateral token |

This source is session-policy v2, which is not deployed and needs review before any deploy. The
testnet address below runs v1, whose transfer-destination guard is bypassable (unrestricted
`approve`, muxed `to` addresses, the router's fee envelope).

### Referral

`attribute(caller: Address, referrer: Address) -> (Address, Address)` requires `caller`'s
authorization and fails with `ReferralError::SelfReferral` (1) when `caller == referrer`. It emits
`Attributed { referee, referrer }` with topics `("attributed", referee, referrer)`, so indexers can
filter on either side, and returns `(caller, referrer)` for simulation pre-flight. The event log is
the record: the contract has no storage, no admin and no constructor.

### Session policy

A trading session key is an ed25519 key registered on the smart account under a `Default` context
rule with a `valid_until`. The policy lets the key call `allowed_contracts` (the markets and the
router) and `transfer` or `approve` the collateral `token`. Every token amount counts against
`spend_limit`, the budget the user sets for the session, wherever it goes; a context past the budget
fails with `SpendLimitExceeded` (4007). Everything else fails closed: the wallet itself, other
tokens, other token functions and contract creation. `get_session(smart_account, context_rule_id)`
returns the config and the amount spent.

The budget counts what the key commits (margin, execution fees, the relay fee cap), not the net
loss. Closing a position does not refill it, and an `approve` counts at its full amount.

## Getting Started

Install Rust through [rustup](https://rustup.rs/) and the
[Stellar CLI 27.1.0 release binary](https://github.com/stellar/stellar-cli/releases/tag/v27.1.0)
on `PATH`.

Run commands from the repository root. Rustup installs Rust 1.98.1 and the
`wasm32v1-none` target from `rust-toolchain.toml`. Make targets use the committed
lockfile; dependency changes must update it explicitly.

Build the contracts with:

```
make
```

Run the unit tests with:

```
make test
```

## Deployment

`make` writes optimized WASM to `target/wasm32v1-none/release/`.

`make release` builds the release WASMs into `wasm/` from a clean checkout and embeds the
checked-out commit as the `source_commit` contract meta. Commit the output in the next commit. No
release has been cut from this workspace yet.

| Contract | Testnet address | Deployed wasm sha256 |
|---|---|---|
| session-policy | CDUUHEXJY3EMQPGGRQS2KVJN7J3RM5HM2QWVUA5AGE3BRUOYW2MZPUAT | a98d1317f918b03af4e23f407eb99711eabda9c64629ae413427e7fa1c4f2135 |
| referral | CAVUAS7CMIXOUXFND77EDNB5OOWBAM4AOAGV4NF6D4JQXAZAAERQDJQQ | 2d459a2180d91b5006ac0154cd97c4f4505165b39971ace0e534c3e549c5dc9d |

The testnet contracts were built from the legacy local sources
(`soroban-smart-account/session-policy` @ 9cded45 and `soroban-referral`), which reproduce the
deployed wasm byte-for-byte with rustc 1.93.1, stellar CLI 25.2.0 and their original lockfiles.
This workspace builds with soroban-sdk 26 and one shared lockfile, so its output does not match
those hashes. The referral functions and their argument types are unchanged; session-policy v2
changes `SessionConfig` and adds `get_session`.
