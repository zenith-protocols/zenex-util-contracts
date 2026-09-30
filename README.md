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
deploy. The workspace depends on OpenZeppelin stellar-contracts at an UNRELEASED, UNAUDITED commit
(`df602b6`, the head of their `v0.9.0` branch) and builds with soroban-sdk 27.0.6; see the fee
forwarder below. The older testnet address below runs v1, whose transfer-destination guard is bypassable
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
names; the forwarder itself cannot be the recipient (`InvalidRecipient`, 6002): OpenZeppelin keeps
such a fee in the contract for a later sweep, and this forwarder has none.

The fee moves through an allowance to the forwarder, collected by OpenZeppelin's own `collect_fee`
with Eager approval. Since
[stellar-contracts#873](https://github.com/OpenZeppelin/stellar-contracts/pull/873) (the fix for
issue #875) the user approves the cap, the forwarder pulls the whole cap, pays the fee and refunds
the rest. The pull consumes the allowance before the target runs, and the user's balance must cover
the cap, not only the fee. That change is only on OpenZeppelin's unreleased `v0.9.0` branch, so
`stellar-fee-abstraction` and `stellar-accounts` are pinned to its head, commit `df602b6`. That
code is **unaudited and unreleased**; move to the crates.io release once v0.9.0 ships. The
forwarder is the direct caller of its target, so `transfer_from` and `burn_from` are also refused as
a target on every contract (`TargetNotAllowed`, 6001): an allowance a wallet grants the forwarder
outside a forward can never be spent through one. OpenZeppelin's example on the same helper leaves
no leftover but still calls any target, so an allowance granted outside a forward is spendable by
anyone's forward there (see the
`the_oz_example_forwarder_spends_an_allowance_granted_outside_a_forward` test).

For a Zenex order the wallet signs one tree rooted at the forwarder call: `approve(forwarder, cap)`
and `market.create_order` with its escrow `transfer`. When the target is the market router, the
router's frames need no wallet authorization and are not part of it.

Testnet run on 2026-09-30 of the OpenZeppelin v0.9.0 build against testnet-v3 (router `CAZ4DNYW…`,
market `CCOIDO46…`, fee recipient `GBIBH5UV…`) through the relay stack (backend, OZ relayer, zenex
plugin in forwarder mode), with real signatures. The relay set the fee and, for `forward_unsafe`,
the keeper and a fresh price after signing. Every run pulled the full 1 USDC cap, refunded the cap
minus the fee, and left the allowance and the forwarder's balance at zero. The session rows run
against the deployed canonical smart account (OpenZeppelin v0.7.1 wasm), which installed this v4
build with an empty parameter:

| Signer | Flow | Transaction |
|---|---|---|
| G-account | open: `forward_unsafe` → `create_and_fill` | 90a7ba073d0039e6307863b528665def720db77c25a12d06d7e99acd8466b06c |
| Smart account (canonical wasm, ed25519 signer) | open: `forward_unsafe` → `create_and_fill` | 31a5ff52dbb4531f945df98efff6d32786cfc20c08b037fcbee9aa6c176a6129 |
| Smart account | limit: `forward` → `multicall` | a2ceb96f8ad63f0439bd645cb66833f3463d767a37daf0373e0c90e5f0ef95f5 |
| Smart account | cancel: `forward` → `multicall` | 6d32639653fd69c0ae0bfa4de7350fdb7c623caa74eebcd1a2c1eb1ae317f18c |
| G-account | close: `forward_unsafe` → `create_and_try_fill` | 2451258dacaa9fda94c364603ea37f3214387bcbada733f6b86fd86eaa55d35e |
| Rule 0 installs session-policy v4 | enable: `forward` → `multicall(add_context_rule)` | 869673aa24354d6787f876aca162f8ce874485631fb1c476718c3b9b1504feb1 |
| Session key under session-policy v4 | open: `forward_unsafe` → `create_and_fill` | 1c2cacae644d8e551c11a873299784289046324e574e828306ec16be28625717 |
| Session key | limit: `forward` → `multicall` | 17ab3d64c191282a823b9f9b038671478d72148d20187d65881bf47b3238ab56 |
| Session key | cancel: `forward` → `multicall` | e54f1c37ea3530989231b3f9fdb2e446639421f6cf142f1430bdf33d81e6fc45 |
| Session key | close: `forward_unsafe` → `create_and_try_fill` | 6fb53f2ac792abeff27e11c5d175c69da253b0480581523cf3baf43c836059ea |

A session-signed forward paying another recipient fails simulation with `Error(Auth,
InvalidAction)` and v4's 4006 in the event log.

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
| session-policy (v4) | CBHJ72ERR2FOSCXIKQ5ZPEZVSJZID7QZAKOTXYNGXUI3EEWRQCRYNCCU | bef197d9c1df2e4d03bfee2621a853d5d10c32be92ccd57fea47ba226ae34d8c |
| session-policy (v1) | CDUUHEXJY3EMQPGGRQS2KVJN7J3RM5HM2QWVUA5AGE3BRUOYW2MZPUAT | a98d1317f918b03af4e23f407eb99711eabda9c64629ae413427e7fa1c4f2135 |
| referral | CAVUAS7CMIXOUXFND77EDNB5OOWBAM4AOAGV4NF6D4JQXAZAAERQDJQQ | 2d459a2180d91b5006ac0154cd97c4f4505165b39971ace0e534c3e549c5dc9d |
| fee-forwarder | CBWLTLD5JJGAVSR2KH3UY42WXH3YYORZW54TA74EIGOPWA74LJGYE6C2 | c7dd9bae43bea382e4890a86c9d41153607fe9ac465a207dccfd2d0b66c0d2ba |

The session-policy v4 row and the fee forwarder are built from this workspace on OpenZeppelin
`df602b6` and soroban-sdk 27.0.6 (commit 1b037f0). v4 is deployed with the forwarder
`CBWLTLD5…E6C2`, the router `CAZ4DNYW…REIY4`, the market `CCOIDO46…2F6U`, USDC `CD4MP2QV…V5S2O` and
the fee recipient `GBIBH5UV…MIKE4`. Superseded testnet deployments: the fee forwarder
`CBR2C7SAO5KRKVHAGVW7X3KPAPEMX3A6G72BH4WX762IZRU6L4JYZR25` (wasm `3fee58c2…0940`, a copy of the
upstream Eager collection on soroban-sdk 26) with its v4 instance
`CDWY6X5ACXOWLVH6YYVFNO2E6NCT2765HNJ3KTWXIAYBJRJPWEDZQP77` (wasm `d9048875…0138`), the fee forwarder `CBLRMGX3TKV57BQDSLV7AYFS6X67NJO7VIREJ2N7UGXRE5DYNV6DHVVC` (wasm
`ed348ebc…6b46`, reset-to-zero collection) with its v4 instance
`CAIH4U3LUAP2HBHVGLG5F2BPMPOOX2IG56OJMBIDU35MFRUVPNAIGUG4`, and the typed predecessor
`CDRHA53H3U35NVQ3PHTUONMG7NSQFFRCQL5QBGTFGTZE5I2735KDLOHR` (wasm `6d4c3791…f956`), a fee layer
fixed to the market router. The session-policy
v1 and referral testnet contracts were built from the legacy local sources
(`soroban-smart-account/session-policy` @ 9cded45 and `soroban-referral`), which reproduce the
deployed wasm byte-for-byte with rustc 1.93.1, stellar CLI 25.2.0 and their original lockfiles.
This workspace builds with soroban-sdk 27 and one shared lockfile, so its output does not match
those hashes. The referral functions and their argument types are unchanged; session-policy v4
replaces v1's interface with a constructor configuration, an empty install parameter and new error
codes.
