# Zenex Util Contracts

Small standalone contracts for Zenex, a perpetual futures exchange on
[Stellar](https://stellar.org) (Soroban). They sit outside the core protocol in
[zenex-contracts](https://github.com/zenith-protocols/zenex-contracts) and do not
link against it; session-policy matches the market, fee-forwarder and router functions it allows by name.
The market router is ported here from zenex-contracts without its fee functions.

## Contracts

| Crate | Description |
|-------|-------------|
| `fee-forwarder` | Generic fee forwarder: pays a relayer in a token under a signed fee cap and recipient, then calls the target |
| `market-router` | Stateless call router: batching (`multicall`, `multicall_try`) and create-and-fill (`create_and_fill`, `create_and_try_fill`); the fee forwarder's target |
| `referral` | Referral attestation: a wallet attests which wallet referred it |
| `session-policy` | Smart-account policy for trading session keys: trade-only, relay fees only through the fee forwarder to a pinned recipient |

This source is session-policy v4; the testnet instance runs its previous build (see Deployment). It
needs review before any mainnet deploy. The workspace depends on OpenZeppelin stellar-contracts at
an UNRELEASED, UNAUDITED commit (`df602b6`, the head of their `v0.9.0` branch) and builds with
soroban-sdk 27.0.6; see the fee forwarder below. The older testnet address below runs v1, whose
transfer-destination guard is bypassable (unrestricted `approve`, muxed `to` addresses, the
router's fee envelope), and whose error codes differ from v4's.

### Referral

`attribute(caller: Address, referrer: Address)` requires `caller`'s authorization and fails with
`ReferralError::SelfReferral` (7001) when `caller == referrer`. It emits `Attributed { referee,
referrer }` with topics `("attributed", referee, referrer)`, so indexers can filter on either side,
and returns nothing: a successful simulation is the pre-flight. The event log is the record: the
contract has no storage, no admin and no constructor.

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

### Market router

Ported from zenex-contracts `market-router` at `c93bc95` (unchanged since the mainnet release
`644a4c4`) with the three `*_with_fee` functions removed. It keeps `multicall`, `multicall_try`,
`create_and_fill` and `create_and_try_fill`; their names, arguments, return shapes and the `Call`
type match the deployed router's spec exactly. It holds no funds and collects no fees: the fee
forwarder collects them and calls these functions as its target. Its tests run against the release
market WASM in `market-router/testdata/market.wasm` (zenex-contracts `wasm/market.wasm`, sha256
`02aa342f…943c`, the testnet-v3 market on chain), including a forwarded fill through
`fee-forwarder` → router → market.

Testnet run on 2026-09-30 through the ported router `CAZFL7XZ…` and its v4 instance `CDUXY6JM…`,
over the same relay stack:

| Signer | Flow | Transaction |
|---|---|---|
| G-account | open: `forward_unsafe` → `create_and_fill` | b261f4f73ef6cbf59365b206d750dd1622e4ca9918f2265e89a7be147dd8daa6 |
| Smart account | open: `forward_unsafe` → `create_and_fill` | f7614e2fd1be5d8552fd4d1832fc9f342c5dc936de1d9001515c26c37de8b2c1 |
| G-account | limit, cancel: `forward` → `multicall` | 95f71e39bc79871a7e28eb510b6e2661a2adca61651d15ce8a3248e225260c1e, ef1a3b07896aa739a663afea6962f847368199e75969ead85d4344616337e1b1 |
| Smart account | limit, cancel: `forward` → `multicall` | d679691b05c1ecf6793228464ab260072a8a08000c9264c944d2a8e54f80ec32, 7a0e2825bfd23a568a4d43f2e5765061af74ea15b555782e2d00c7c9f054ae34 |
| G-account, smart account | close: `forward_unsafe` → `create_and_try_fill` | 6dc0126cf84af7790502e5b2074b5c06b08be72b31dea017be9ad0450467246f, 29e4b8f38cfb7ce6cc9dd828ffb8f424c9f6839e63d868c751456d822d3524c4 |
| Rule 0 installs v4 | enable: `forward` → `multicall(add_context_rule)` | 60d2f5d86ff54832bac296b8babeb86dcdbb0079e88fbb499620344fbbc760f7 |
| Session key under v4 | open: `forward_unsafe` → `create_and_fill` | 120c53c11fb6fdfc18a5292a533d58e3ce8e188d70d7947ff0cdc197d73d746b |
| Session key | limit, cancel: `forward` → `multicall` | 86a81be2d3cac84fb6828f19028365b00358529a102b14330257c0e2de3a006e, 855fff85385f5d6227c6a09b727bd4bde249547acdf3e5dd22ba8b7bba156ba7 |
| Session key | close: `forward_unsafe` → `create_and_try_fill` | c06424dbfef919ef20be374ba4abaa495a5ea27b4d7b0ea70f30c85c4d6804e5 |
| Rule 0 | disable: `forward` → `multicall(remove_context_rule)` | db5562f4900db6677e29eda1f7f8ab8c337b865e1ffce65c8bf626b1fa0f4ce2 |

### Session policy

A trading session key is an ed25519 key registered on the smart account under a `Default` context
rule with a `valid_until`. The constructor fixes the fee forwarder, the router, the markets, the
collateral token and the fee recipient, one instance-storage entry each; there is no admin and no
per-account state, a rule installs the policy with an empty parameter, and install stores nothing.
The constructor rejects empty markets or any repeated address (`InvalidConfig` 4001).

Every signer of the session rule must have signed (`SignerNotAuthenticated` 4007). The wallet checks
a rule's signers itself only when the rule has no policies; with one, it leaves the check to the
policy, so without it an authorization carrying no signature at all, naming the session rule, would
pass. `session-policy/tests/signers.rs` runs that attack against the real canonical wallet and
ed25519 verifier WASMs in `session-policy/testdata/`. With the session key's signature, the key may
sign:

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
diagnostic event log.

The configuration sits under the instance keys `forwarder`, `router`, `markets`, `token` and
`recipient`, and `enforce` reads only the entries its branch checks. There is no getter, because
every `enforce` instantiates the whole module and pays for each export: `stellar contract read --id
<policy>` prints the instance entry.

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

CI (`.github/workflows/ci.yml`) runs `cargo fmt --all -- --check` and `cargo test --locked
--workspace` on every push and pull request. The tests need no built WASM: the only WASMs they load
are committed testdata, the market in `market-router/testdata/` and the canonical wallet and ed25519
verifier in `session-policy/testdata/`.

## Error codes

A contract error surfaces as `Error(Contract, #code)` whichever contract in the call tree raised it,
so each contract here takes codes no neighbour uses. The codes that can meet in a Zenex call tree:

| Codes | Raised by |
|---|---|
| 1–15 | the host's built-in contracts, such as the Stellar Asset Contract |
| 1, 600–900 | zenex-contracts (governance, factory, market, oracle, strategy vault, treasury) |
| 100–411, 1000–1502, 2000–2203 | OpenZeppelin `stellar-tokens`, `stellar-contract-utils` and `stellar-access`, in zenex-contracts |
| 3000–3227 | OpenZeppelin `stellar-accounts`: the smart account, its verifiers and policies |
| 4001–4007 | `session-policy` |
| 5000–5006 | OpenZeppelin `stellar-fee-abstraction` |
| 6001–6002 | `fee-forwarder` |
| 7001 | `referral` |

`market-router` defines none: a failing call traps with its own error. OpenZeppelin's governance
(4000–4104) and zk-email (6000–6001) modules reuse two of these ranges; nothing here links them.

## Releases

Pushing a version tag (`v1.2.3`) runs `.github/workflows/release.yml`: one job per contract on
stellar.expert's reusable [soroban-build-workflow](https://github.com/stellar-expert/soroban-build-workflow),
pinned to its v27.0.0 commit. Each job builds the contract with stellar-cli 27.0.0, publishes a
GitHub release with the WASM, attests its build provenance and submits the WASM hash to
stellar.expert, which then shows a contract running that exact WASM as verified against this source.

- Verification needs the repository to be public (planned): stellar.expert reads the source, and
  GitHub attests builds of private repositories only on Enterprise Cloud, so until then a tag run
  publishes the releases and fails at the attest step.
- stellar.expert's intake currently drops submissions while the job stays green
  ([soroban-build-workflow#9](https://github.com/stellar-expert/soroban-build-workflow/issues/9)).
  The attestation side is expected to work once stellar.expert fixes it, with no change here.
- Deploy the WASM attached to the release. It embeds `source_repo` and stellar-cli 27.0.0 as
  `cliver`, so its hash differs from `make` output.
- Tag the head of `main`: each release's own tag (`<tag>_<package>_pkg0.0.0_cli27.0.0`) is created
  at the default branch head. Leave GitHub's immutable releases off; the release attestation they
  add may break stellar.expert's matching
  ([soroban-build-workflow#8](https://github.com/stellar-expert/soroban-build-workflow/issues/8)).

## Deployment

`make` writes optimized WASM to `target/wasm32v1-none/release/`.

`make release` builds the release WASMs into `wasm/` from a clean checkout and embeds the
checked-out commit as the `source_commit` contract meta. Commit the output in the next commit. A
verifiable release comes from a version tag instead (see Releases). No release has been cut from
this workspace yet.

| Contract | Testnet address | Deployed wasm sha256 |
|---|---|---|
| market-router | CAZFL7XZGYND5MLKQB4SCGY7OUAML6Z4DW2CU7BUXNMRWJ4M72C45EAW | e60009c4c26f784fa878a55031aeb857e5edfb70a61347e037019b6ab946f283 |
| session-policy (v4) | CDUXY6JMMWKDI7WZBBD4ENKKPJZXN7KTUBAWMGNSJPP6O5JEUNXGE4FO | bef197d9c1df2e4d03bfee2621a853d5d10c32be92ccd57fea47ba226ae34d8c |
| session-policy (v1) | CDUUHEXJY3EMQPGGRQS2KVJN7J3RM5HM2QWVUA5AGE3BRUOYW2MZPUAT | a98d1317f918b03af4e23f407eb99711eabda9c64629ae413427e7fa1c4f2135 |
| referral | CAVUAS7CMIXOUXFND77EDNB5OOWBAM4AOAGV4NF6D4JQXAZAAERQDJQQ | 2d459a2180d91b5006ac0154cd97c4f4505165b39971ace0e534c3e549c5dc9d |
| fee-forwarder | CBWLTLD5JJGAVSR2KH3UY42WXH3YYORZW54TA74EIGOPWA74LJGYE6C2 | c7dd9bae43bea382e4890a86c9d41153607fe9ac465a207dccfd2d0b66c0d2ba |

The session-policy v4 row and the fee forwarder are built from this workspace on OpenZeppelin
`df602b6` and soroban-sdk 27.0.6 (commit 1b037f0), the market router from commit f2bd687. The
session-policy source has since moved its configuration to per-value instance keys and dropped
`get_config`, with the same constructor and `enforce` rules, so it no longer builds the deployed v4
WASM. v4 is deployed with the forwarder `CBWLTLD5…E6C2`, the ported router `CAZFL7XZ…5EAW`, the market
`CCOIDO46…2F6U`, USDC `CD4MP2QV…V5S2O` and the fee recipient `GBIBH5UV…MIKE4`. Superseded testnet
deployments: the v4 instance `CBHJ72ERR2FOSCXIKQ5ZPEZVSJZID7QZAKOTXYNGXUI3EEWRQCRYNCCU` (same wasm),
pinned to the zenex-contracts router `CAZ4DNYW…REIY4`; the fee forwarder
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
those hashes. `attribute` keeps its arguments, but v1 returns `(caller, referrer)` and fails
with 1; session-policy v4 replaces v1's interface with a constructor configuration, an empty install
parameter and new error codes.
