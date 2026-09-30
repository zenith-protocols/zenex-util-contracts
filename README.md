# Zenex Util Contracts

Small standalone contracts for Zenex, a perpetual futures exchange on
[Stellar](https://stellar.org) (Soroban). They sit outside the core protocol in
[zenex-contracts](https://github.com/zenith-protocols/zenex-contracts) and do not
link against it; session-policy matches the market, fee-forwarder and router functions it allows by name.

## Contracts

| Crate | Description |
|-------|-------------|
| `fee-forwarder` | Generic fee forwarder: pays a relayer in a token under a signed fee cap and recipient, then calls the target |
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

### Fee forwarder

A stateless fee-abstraction primitive with OpenZeppelin's shape: deploy once, anyone uses it. Both
entry points take `(fee_token, fee_amount, max_fee_amount, expiration_ledger, target_contract,
target_fn, target_args, user, fee_recipient)`, collect `fee_amount` from `user` to `fee_recipient`,
and then call `target_contract.target_fn(target_args)` and return its result.

- `forward` binds `user` to `(fee_token, max_fee_amount, expiration_ledger, fee_recipient,
  target_contract, target_fn, target_args)` with `require_auth_for_args`.
- `forward_unsafe` binds the same projection without `target_args`, so the relayer can refresh them
  after signing (a fresh price). The target flow must require the user's own authorization on every
  call that moves the user's funds; a Zenex `create_order` does.

`fee_amount` is the relayer's, at most the cap and above zero. Unlike OpenZeppelin's example, the
user signs `fee_recipient` (index 3 of both projections), so a signature pays only the recipient it
names. The fee moves through an allowance to the forwarder, and the forwarder is the direct caller of
its target, so two rules keep that allowance from being spent by anyone else: the leftover is reset
to zero right after the pull, before the target runs, and `transfer_from` and `burn_from` are refused
as a target on every contract (`TargetNotAllowed`, 6001). The OpenZeppelin example keeps the
leftover and calls any target, so another user's forward can spend it (see the
`the_oz_example_forwarder_leaks_a_leftover_to_anyone` test).

For a Zenex order the wallet signs one tree rooted at the forwarder call: `approve(forwarder, cap)`,
`approve(forwarder, 0)`, and `market.create_order` with its escrow `transfer`. When the target is the
market router, the router's frames need no wallet authorization and are not part of it.

Testnet run on 2026-09-30 against testnet-v3 (router `CAZ4DNYW…`, market `CCOIDO46…`, fee
recipient `GBIBH5UV…`), with real signatures. The relayer set the fee and, for `forward_unsafe`, the
keeper and a fresh price after signing; every run left the allowance at zero:

| Trader | Flow | Transaction |
|---|---|---|
| G-account | open: `forward_unsafe` → `create_and_fill` | b2f8a44f53380e5b50266dcd827a7339f621b215d3368757259fb89e01eca47a |
| Smart account (canonical wasm, ed25519 signer) | open: `forward_unsafe` → `create_and_fill` | 6396b0afa29eccead783e49ea88222ab649cce018ba5ae9bdb3601e236cfb541 |
| G-account | limit: `forward` → `multicall` | 9dd15fea8de217e70cf1e9d38f2b6dfe738dacf790c114b643b8e79ae88cafdb |
| Smart account | limit: `forward` → `multicall` | d209f20ddcc917b8f5e1df25c749f5d82f50cc7e872cf15b0453a939a3759aaf |
| G-account | cancel: `forward` → `multicall` | 78652acb81f53b361f80d447d093860c45c57b61123a7172dc3da38ef5033b64 |
| Smart account | cancel: `forward` → `multicall` | 70975c5bcad1f6cce994bbf46c2c3f727081daa1a362888199f3e0b7cc29c8c2 |
| G-account | close: `forward_unsafe` → `create_and_try_fill` | 4a02de86c51a61432a87646b7e44b1e582bf882c0f0046fc54d962803558966b |
| Smart account | close: `forward_unsafe` → `create_and_try_fill` | 17197df29d31980cbc7676b61d361d9bf100dbd830b593bf3d03f28b4cf8daac |

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
| fee-forwarder | CBLRMGX3TKV57BQDSLV7AYFS6X67NJO7VIREJ2N7UGXRE5DYNV6DHVVC | ed348ebc7ff58d78cafa1bde0e9df568418e17171cbf8f2bb26468f1d6496b46 |

The session-policy v4 row and the fee forwarder are built from this workspace. v4 (commit 32522a7)
is deployed with the forwarder `CBLRMGX3…DHVVC`, the router `CAZ4DNYW…REIY4`, the market
`CCOIDO46…2F6U`, USDC `CD4MP2QV…V5S2O` and the fee recipient `GBIBH5UV…MIKE4`.
`CDRHA53H3U35NVQ3PHTUONMG7NSQFFRCQL5QBGTFGTZE5I2735KDLOHR` (wasm `6d4c3791…f956`) is the fee
forwarder's superseded typed predecessor, a fee layer fixed to the market router. The session-policy
v1 and referral testnet contracts were built from the legacy local sources
(`soroban-smart-account/session-policy` @ 9cded45 and `soroban-referral`), which reproduce the
deployed wasm byte-for-byte with rustc 1.93.1, stellar CLI 25.2.0 and their original lockfiles.
This workspace builds with soroban-sdk 26 and one shared lockfile, so its output does not match
those hashes. The referral functions and their argument types are unchanged; session-policy v4
replaces v1's interface with a constructor configuration, an empty install parameter and new error
codes.
