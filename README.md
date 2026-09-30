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
names; the forwarder itself cannot be the recipient (`InvalidRecipient`, 6002).

The fee moves through an allowance to the forwarder, collected the way OpenZeppelin's Eager approval
does it after [stellar-contracts#873](https://github.com/OpenZeppelin/stellar-contracts/pull/873)
(the fix for issue #875): the user approves the cap, the forwarder pulls the whole cap, pays the fee
and refunds the rest. The pull consumes the allowance before the target runs, and the user's balance
must cover the cap, not only the fee. That change is only on OpenZeppelin's unreleased v0.9.0 branch
(soroban-sdk 27), so it is copied here against the released 0.7.2 crate until v0.9.0 ships. The
forwarder is the direct caller of its target, so `transfer_from` and `burn_from` are also refused as
a target on every contract (`TargetNotAllowed`, 6001): an allowance a wallet grants the forwarder
outside a forward can never be spent through one. The released OpenZeppelin example keeps the
leftover and calls any target, so another user's forward can spend it (see the
`the_oz_example_forwarder_leaks_a_leftover_to_anyone` test).

For a Zenex order the wallet signs one tree rooted at the forwarder call: `approve(forwarder, cap)`
and `market.create_order` with its escrow `transfer`. When the target is the market router, the
router's frames need no wallet authorization and are not part of it.

Testnet run on 2026-09-30 against testnet-v3 (router `CAZ4DNYW…`, market `CCOIDO46…`, fee
recipient `GBIBH5UV…`) through the relay stack (backend, OZ relayer, zenex plugin in forwarder mode),
with real signatures. The relay set the fee and, for `forward_unsafe`, the keeper and a fresh price
after signing. Every run pulled the full 1 USDC cap, refunded the cap minus the fee, and left the
allowance and the forwarder's balance at zero:

| Signer | Flow | Transaction |
|---|---|---|
| G-account | open: `forward_unsafe` → `create_and_fill` | a0bafbc060a7d862d062b4f29a320561bfc6b78a1054942744ac5193a862b879 |
| Smart account (canonical wasm, ed25519 signer) | limit: `forward` → `multicall` | de7779eaeb6de31ca687b8126bfc62104dd3cb7ec1c7c5af8fce95a12540e9de |
| Smart account | cancel: `forward` → `multicall` | 02a27eb533d272b5c1c180a906a0fc491a33c491350a20fb0af69248ce3447f7 |
| G-account | close: `forward_unsafe` → `create_and_try_fill` | cb4e049456a3122adb4b9ebde4a6c68c2b133ebb54819927df15d93703d1fbf8 |
| Session key under session-policy v4 | open: `forward_unsafe` → `create_and_fill` | 3b6ed0359a458efc5c6e4535770f165506a183ba4ecc3827a7d90ac9e933da74 |
| Session key | limit: `forward` → `multicall` | ae2f1fcf70a2ccb4dea552f1251d9436d1fe69fb0c086417697f382eff7dc48b |
| Session key | cancel: `forward` → `multicall` | 0151b683759ee39898e5453e2898f773e13e9786f62f7b4f3a493b81fcb6bb95 |
| Session key | close: `forward_unsafe` → `create_and_try_fill` | 5db8f100e6f3604e7442cfa571e21efbf790a8975d08e762517be8460e6a44df |

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
| session-policy (v4) | CDWY6X5ACXOWLVH6YYVFNO2E6NCT2765HNJ3KTWXIAYBJRJPWEDZQP77 | d90488750ab71d43b649d02e31cdbd8eabb2f968acfdf8945579d9b404f00138 |
| session-policy (v1) | CDUUHEXJY3EMQPGGRQS2KVJN7J3RM5HM2QWVUA5AGE3BRUOYW2MZPUAT | a98d1317f918b03af4e23f407eb99711eabda9c64629ae413427e7fa1c4f2135 |
| referral | CAVUAS7CMIXOUXFND77EDNB5OOWBAM4AOAGV4NF6D4JQXAZAAERQDJQQ | 2d459a2180d91b5006ac0154cd97c4f4505165b39971ace0e534c3e549c5dc9d |
| fee-forwarder | CBR2C7SAO5KRKVHAGVW7X3KPAPEMX3A6G72BH4WX762IZRU6L4JYZR25 | 3fee58c2724e0321b780ff69c33276898ac36406ab1a2eaf9fb942e0d25b0940 |

The session-policy v4 row and the fee forwarder are built from this workspace. v4 (commit 32522a7)
is deployed with the forwarder `CBR2C7SA…ZR25`, the router `CAZ4DNYW…REIY4`, the market
`CCOIDO46…2F6U`, USDC `CD4MP2QV…V5S2O` and the fee recipient `GBIBH5UV…MIKE4`. Superseded testnet
deployments: the fee forwarder `CBLRMGX3TKV57BQDSLV7AYFS6X67NJO7VIREJ2N7UGXRE5DYNV6DHVVC` (wasm
`ed348ebc…6b46`, reset-to-zero collection) with its v4 instance
`CAIH4U3LUAP2HBHVGLG5F2BPMPOOX2IG56OJMBIDU35MFRUVPNAIGUG4`, and the typed predecessor
`CDRHA53H3U35NVQ3PHTUONMG7NSQFFRCQL5QBGTFGTZE5I2735KDLOHR` (wasm `6d4c3791…f956`), a fee layer
fixed to the market router. The session-policy
v1 and referral testnet contracts were built from the legacy local sources
(`soroban-smart-account/session-policy` @ 9cded45 and `soroban-referral`), which reproduce the
deployed wasm byte-for-byte with rustc 1.93.1, stellar CLI 25.2.0 and their original lockfiles.
This workspace builds with soroban-sdk 26 and one shared lockfile, so its output does not match
those hashes. The referral functions and their argument types are unchanged; session-policy v4
replaces v1's interface with a constructor configuration, an empty install parameter and new error
codes.
