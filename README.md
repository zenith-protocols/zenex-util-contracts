# Zenex Util Contracts

Small standalone contracts for Zenex, a perpetual futures exchange on
[Stellar](https://stellar.org) (Soroban). They sit outside the core protocol in
[zenex-contracts](https://github.com/zenith-protocols/zenex-contracts) and do not
link against it; session-policy matches the token and fee-forwarder functions it checks by name.
The market router is ported here from zenex-contracts without its fee functions.

## Contracts

| Crate | Description |
|-------|-------------|
| `fee-forwarder` | Generic fee forwarder: pays a relayer in a token under a signed fee cap and recipient, then calls the target |
| `market-router` | Stateless call router: batching (`multicall`, `multicall_try`) and create-and-fill (`create_and_fill`, `create_and_try_fill`); the fee forwarder's target |
| `referral` | Referral attestation: a wallet attests which wallet referred it |
| `session-policy` | Smart-account policy for trading session keys: market calls, token escrow into markets, and relay fees only through the fee forwarder to a pinned recipient |

The testnet deployments (see Deployment) are built from this source, including the session policy's
signer check.
It needs review before any mainnet deploy. The workspace depends on OpenZeppelin stellar-contracts at
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
- `forward_dynamic` binds the same projection without `target_args`, so the relayer can refresh
  them after signing (a fresh price). The target flow must require the user's own authorization on
  every call that moves the user's funds; a Zenex `create_order` does.

`forward_dynamic` was called `forward_unsafe` until it was renamed for how it reads in a signing
prompt; the signed projection is unchanged. The 2026-09-30 runs below used earlier deployments that
expose `forward_unsafe`; the current testnet deployment (see Deployment) exposes `forward_dynamic`.
relayer-plugin-zenex PR #25 and zenex-trade PR #79 still use the old name and need the same rename
when they are next touched.

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

Both entry points fail with `TargetNotAllowed` (6001), `InvalidRecipient` (6002), OpenZeppelin's
`InvalidFeeBounds` when `fee_amount` is not above zero or exceeds the cap, `InvalidUser` when `user`
is the forwarder, and the token's own error when `user` holds less than the cap. A successful call
emits only OpenZeppelin's `["fee_collected", user, recipient]` with data `[token, amount]`; there is
no forward event, since the call data is already in the transaction and the target emits its own.

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
`644a4c4`) with the three `*_with_fee` functions removed. zenex-contracts is private, so those
commits cannot be checked from here; the market WASM the tests use can be checked on chain by its
hash. It keeps `multicall`, `multicall_try`,
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
rule with a `valid_until`. The policy looks only at authorizations: every call that asks for the
wallet's signature reaches `enforce` as one context, and those live in three contracts, the
forwarder, the markets and the token.

The constructor takes `(forwarder, markets, token, fee_recipient)` and stores one instance entry
each (`forwarder`, `markets`, `token`, `recipient`). It rejects empty markets or any repeated address
(`InvalidConfig` 4001). There is no admin, no per-account state and no getter: a rule installs the
policy with an empty parameter, install stores nothing, and `stellar contract read --id <policy>`
prints the configuration.

`enforce` allows a context only when:

- every signer of the session rule signed (`SignerNotAuthenticated` 4007). A wallet checks a rule's
  signers itself only when the rule has no policies; with one, it leaves the check here, so without
  it an authorization carrying no signature at all would pass. `session-policy/tests/signers.rs`
  runs that attack against the real canonical wallet and ed25519 verifier WASMs in
  `session-policy/testdata/`;
- on the forwarder, the signed `fee_recipient` (index 3 of both projections) is the pinned recipient
  (`ForwardNotAllowed` 4006). The fee leaves through the forwarder's own `transfer_from`, which never
  reaches the policy, so that is the one thing to pin; whatever a forward calls, each call that
  needs the wallet is a context of its own;
- on a market, any function: a market call that needs the wallet acts on its own funds and pays
  back to it;
- on the token, `transfer` goes into a market (`TransferNotAllowed` 4004, muxed destinations
  included) or `approve` names the forwarder, at any amount (`ApproveNotAllowed` 4005); any other
  token function fails (`FunctionNotAllowed` 4003).

Everything else fails with `ContractNotAllowed` (4002): the wallet itself, the vault, any router,
other tokens and contract creation. A fee in another token fails at that token's `approve`. A
rejected context shows as `Error(Auth, InvalidAction)`, with the policy's code in the diagnostic
event log.

A session key can deposit into the vault but never withdraw: a redeem moves the wallet's shares
with `vault.transfer`, which is refused. A stolen key can trade on the markets, which mostly moves
losses and fees to the vault, plus the execution fee of each order it fills itself, and can pay
relay fees only to the pinned recipient. It cannot withdraw, move other tokens or touch the wallet.
Markets are upgradeable through governance, so a future market function that asks for the wallet's
authorization is allowed automatically.

The testnet session policy (see Deployment) is built from this source.

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
| market-router | CADNB773DWGI2KZMY7D7XL4PHOR7ICU45YPTOL4JJLXDUYYYTIO6V5QT | e8ab1b890ea258c010604ad8626a1529125018a863003fc2979c7bf0277e2825 |
| session-policy | CAX22IBJ33YLHCKNMJZ2B5HV3QVG6XH3U5Z66BUSAHY6ALIP72QBWEJK | 49f3d545d3eba03265183b16c7de432d3b1db8d9460c3af94b44ef8a215940a6 |
| session-policy (v1) | CDUUHEXJY3EMQPGGRQS2KVJN7J3RM5HM2QWVUA5AGE3BRUOYW2MZPUAT | a98d1317f918b03af4e23f407eb99711eabda9c64629ae413427e7fa1c4f2135 |
| referral | CAVUAS7CMIXOUXFND77EDNB5OOWBAM4AOAGV4NF6D4JQXAZAAERQDJQQ | 2d459a2180d91b5006ac0154cd97c4f4505165b39971ace0e534c3e549c5dc9d |
| fee-forwarder | CDUDWXU3UBMW6NDXYJGLBOSJMAQE4ESVR2UF64SUJ45NOO6CAOQSQEMY | 2e333ddb9da76d1fd3d19390bf03d2c17a9ce629b0537299d05497ceee347b26 |

The market-router, session-policy and fee-forwarder rows are built from commit bfe0b72
(soroban-sdk 27.0.6, OpenZeppelin `df602b6`) and match this source. The session policy is deployed
with the forwarder `CDUDWXU3…QEMY`, the market `CCOIDO46…2F6U`, USDC `CD4MP2QV…V5S2O` and the fee
recipient `GBIBH5UV…MIKE4`.

Testnet run on 2026-10-01 of those three through the relay stack (backend, OZ relayer, zenex plugin
in forwarder mode), with real signatures. Every trade went through this forwarder and router, paid
its fee to `GBIBH5UV…`, refunded the rest of the 1 USDC cap, left the allowance and the forwarder's
balance at zero, and the forwarder emitted only `fee_collected`. The smart account is the deployed
canonical wasm with an ed25519 signer on rule 0; the session key runs under the policy above:

| Signer | Flow | Transaction |
|---|---|---|
| G-account | open: `forward_dynamic` → `create_and_fill` | 328135e3db63e28fe7a3e60960fe51019b137729971dfc02413ef447675b7b0f |
| Smart account | open: `forward_dynamic` → `create_and_fill` | 6d7dd5aa1dbaa2202eb523e8c3db513162e903d850c1069ebe63eb108acad00f |
| G-account | limit: `forward` → `multicall` | 94e91a644b7d5d39864123dec4449f2425f85135b643804aee493855afac08ea |
| G-account | cancel: `forward` → `multicall` | 74671ba7fd4696127534692d5b195917facdc62692bed8d4cc08e2d417a29ec4 |
| Smart account | limit: `forward` → `multicall` | 06f23bd6f5af157f793ffe9f7a44b9944150c780307244673c50ec2addeb6126 |
| Smart account | cancel: `forward` → `multicall` | fda7a13029ff36b39b31a255f281758ec3d1966648e1599f799a7a014bf677a3 |
| G-account | close: `forward_dynamic` → `create_and_try_fill` | 75258c467ff3e3a143f2b15e2f2d7f677ba20573af75d46ed221ddf2a1f5d76f |
| Smart account | close: `forward_dynamic` → `create_and_try_fill` | f96f02a4265a8b3a61ff270a247cbfbbcf2c56b648f5ab56639f35b5789cdb5b |
| Rule 0 installs the session policy | enable: `forward` → `multicall(add_context_rule)` | cd9fe427399ded13018ae251bb7a1742f313fbbabcfd5511a08f901a7b8c02c8 |
| Session key | open: `forward_dynamic` → `create_and_fill` | cbecc0eb956b0c60a2e8eb5e3aaf4d717a72f3efe06fde5dc8bdf1119557d850 |
| Session key | limit: `forward` → `multicall` | 7b1a7419121cf8a515ddbd56de5056c25018afbd964ec90bebb912232f6361f2 |
| Session key | cancel: `forward` → `multicall` | b2eeec94e1ba130646bd48bf99507097e4988b97d6dfe32d3c7cea04b430a094 |
| Session key | close: `forward_dynamic` → `create_and_try_fill` | 8e06adb771f0a17053269a509cb5c2c7401c586f216912d38a3b3e4d46105290 |
| Rule 0 removes the session rule | disable: `forward` → `multicall(remove_context_rule)` | 8eb0345abb74bafed122bab60eae567a986b1df76b2d30fe0444b80f15498da5 |

The relay rejects a submit whose fee recipient changed after prepare (HTTP 400), and a session-signed
forward paying another recipient fails simulation with `Error(Auth, InvalidAction)` and the policy's
4006 in the event log.

Superseded testnet deployments: the market router `CAZFL7XZGYND5MLKQB4SCGY7OUAML6Z4DW2CU7BUXNMRWJ4M72C45EAW`
(wasm `e60009c4…f283`, before the helpers were inlined); the session-policy v4 instance
`CDUXY6JMMWKDI7WZBBD4ENKKPJZXN7KTUBAWMGNSJPP6O5JEUNXGE4FO` (wasm `bef197d9…4d8c`), which predates the
signer check and must not be used; the fee forwarder
`CBWLTLD5JJGAVSR2KH3UY42WXH3YYORZW54TA74EIGOPWA74LJGYE6C2` (wasm `c7dd9bae…d2ba`, `forward_unsafe` and
the forward event); the v4 instance `CBHJ72ERR2FOSCXIKQ5ZPEZVSJZID7QZAKOTXYNGXUI3EEWRQCRYNCCU` (same wasm),
pinned to the zenex-contracts router `CAZ4DNYW…REIY4`; the fee forwarder
`CBR2C7SAO5KRKVHAGVW7X3KPAPEMX3A6G72BH4WX762IZRU6L4JYZR25` (wasm `3fee58c2…0940`, a copy of the
upstream Eager collection on soroban-sdk 26) with its v4 instance
`CDWY6X5ACXOWLVH6YYVFNO2E6NCT2765HNJ3KTWXIAYBJRJPWEDZQP77` (wasm `d9048875…0138`), the fee forwarder `CBLRMGX3TKV57BQDSLV7AYFS6X67NJO7VIREJ2N7UGXRE5DYNV6DHVVC` (wasm
`ed348ebc…6b46`, reset-to-zero collection) with its v4 instance
`CAIH4U3LUAP2HBHVGLG5F2BPMPOOX2IG56OJMBIDU35MFRUVPNAIGUG4`, and the typed predecessor
`CDRHA53H3U35NVQ3PHTUONMG7NSQFFRCQL5QBGTFGTZE5I2735KDLOHR` (wasm `6d4c3791…f956`), a fee layer
fixed to the market router. The session-policy
v1 and referral testnet contracts predate this workspace; their sources were imported in commit
`20719fe`, which is the source of record for them here. This workspace builds with soroban-sdk 27
and one shared lockfile, so its output does not match those deployed hashes. `attribute` keeps its
arguments, but v1 returns `(caller, referrer)` and fails with 1; session-policy v4 replaces v1's
interface with a constructor configuration, an empty install parameter and new error codes.
