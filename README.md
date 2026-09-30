# Zenex Util Contracts

Small standalone contracts for Zenex, a perpetual futures exchange on
[Stellar](https://stellar.org) (Soroban). They sit outside the core protocol in
[zenex-contracts](https://github.com/zenith-protocols/zenex-contracts) and do not
link against it; session-policy matches the market, fee-forwarder and router functions it allows by name.

## Contracts

| Crate | Description |
|-------|-------------|
| `referral` | Referral attestation: a wallet attests which wallet referred it |
| `session-policy` | Smart-account policy for trading session keys: trade-only, relay fees only through the fee forwarder to a pinned recipient |

This source is session-policy v4, deployed on testnet for review; it needs review before any mainnet
deploy. The older testnet address below runs v1, whose transfer-destination guard is bypassable
(unrestricted `approve`, muxed `to` addresses, the router's fee envelope), and whose error codes
differ from v4's.

### Referral

`attribute(caller: Address, referrer: Address) -> (Address, Address)` requires `caller`'s
authorization and fails with `ReferralError::SelfReferral` (1) when `caller == referrer`. It emits
`Attributed { referee, referrer }` with topics `("attributed", referee, referrer)`, so indexers can
filter on either side, and returns `(caller, referrer)` for simulation pre-flight. The event log is
the record: the contract has no storage, no admin and no constructor.

### Session policy

A trading session key is an ed25519 key registered on the smart account under a `Default` context
rule with a `valid_until`. The constructor fixes the fee forwarder, the router, the markets, the
collateral token and the fee recipient; there is no admin and no storage, and a rule installs the
policy with an empty parameter. The key may sign:

- `forward` / `forward_unsafe` on the forwarder, when the signed projection
  `[fee_token, max_fee_amount, expiration_ledger, fee_recipient, target_contract, target_fn(, target_args)]`
  pays the token to the pinned recipient and targets the router's `multicall`, `create_and_fill` or
  `create_and_try_fill`;
- `create_order`, `cancel_order` and `claim_credit` on the markets;
- `transfer` of the token into a market (the order escrow);
- `approve` of the token to the forwarder, at any amount.

Everything else fails closed (`ContractNotAllowed` 4002, `FunctionNotAllowed` 4003,
`TransferNotAllowed` 4004, `ApproveNotAllowed` 4005, `ForwardNotAllowed` 4006): the wallet itself,
the router directly, the vault, other tokens, muxed or non-market destinations, and contract
creation. A rejected context shows as `Error(Auth, InvalidAction)`, with the policy's code in the
diagnostic event log. `get_config()` returns the deploy-time configuration.

A forwarder allowance needs no cap: the forwarder refuses `transfer_from` and `burn_from` targets,
so only its own fee step can spend it, under the wallet's root authorization, paying the recipient
this policy pins. A stolen key can trade on the markets, which mostly moves losses and fees to the
vault plus the execution fee of each order it fills itself, and can pay relay fees only to the
pinned recipient. It cannot withdraw, move other tokens, or touch the wallet, the router, the vault
or other contracts.

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
| session-policy (v4) | CAIH4U3LUAP2HBHVGLG5F2BPMPOOX2IG56OJMBIDU35MFRUVPNAIGUG4 | d90488750ab71d43b649d02e31cdbd8eabb2f968acfdf8945579d9b404f00138 |
| session-policy (v1) | CDUUHEXJY3EMQPGGRQS2KVJN7J3RM5HM2QWVUA5AGE3BRUOYW2MZPUAT | a98d1317f918b03af4e23f407eb99711eabda9c64629ae413427e7fa1c4f2135 |
| referral | CAVUAS7CMIXOUXFND77EDNB5OOWBAM4AOAGV4NF6D4JQXAZAAERQDJQQ | 2d459a2180d91b5006ac0154cd97c4f4505165b39971ace0e534c3e549c5dc9d |

The v4 row was built from this workspace (commit 32522a7) and deployed with the forwarder
`CBLRMGX3…DHVVC`, the router `CAZ4DNYW…REIY4`, the market `CCOIDO46…2F6U`, USDC `CD4MP2QV…V5S2O`
and the fee recipient `GBIBH5UV…MIKE4`. The other testnet contracts were built from the legacy local sources
(`soroban-smart-account/session-policy` @ 9cded45 and `soroban-referral`), which reproduce the
deployed wasm byte-for-byte with rustc 1.93.1, stellar CLI 25.2.0 and their original lockfiles.
This workspace builds with soroban-sdk 26 and one shared lockfile, so its output does not match
those hashes. The referral functions and their argument types are unchanged; session-policy v4
replaces v1's interface with a constructor configuration, an empty install parameter and new error
codes.
