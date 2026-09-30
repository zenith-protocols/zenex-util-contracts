# Zenex Util Contracts

Small standalone contracts for Zenex, a perpetual futures exchange on
[Stellar](https://stellar.org) (Soroban). They sit outside the core protocol in
[zenex-contracts](https://github.com/zenith-protocols/zenex-contracts) and do not
depend on its interfaces.

## Contracts

| Crate | Description |
|-------|-------------|
| `fee-forwarder` | Relayer fee layer in front of the market router: a capped fee to a fixed recipient, then the router's non-fee flow |
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

### Fee forwarder

A relayer fee layer in front of the market router. Each entry point (`multicall_with_fee`,
`create_and_fill_with_fee`, `create_and_try_fill_with_fee`) binds the user's authorization to
`(calls, max_fee_amount, fee_expiration)` with `require_auth_for_args`, collects `fee_amount` into
the recipient fixed at deploy, wipes the leftover allowance, and then runs the router's matching
non-fee flow and returns its result. The submitter sets `fee_amount` (at most the cap), `keeper` and
`price` after the user signs. The constructor fixes the router, the fee token and the recipient; there
is no admin. This is the one contract here that depends on a zenex-contracts interface: the router's
three non-fee flows.

The wallet signs one tree rooted at the forwarder call: `approve(forwarder, cap)`,
`approve(forwarder, 0)`, and `market.create_order` with its escrow `transfer`. The router's frames
need no wallet authorization and are not part of it. An allowance to the forwarder cannot be spent
through the router's generic calls, because a contract is authorized only for the calls it makes
directly, and the forwarder wipes it before the router runs.

Testnet run on 2026-09-30 against testnet-v3 (router `CAZ4DNYW…`, market `CCOIDO46…`, fee
recipient `GBIBH5UV…`), with real signatures and the relayer setting the fee and the price after
signing:

| Trader | Flow | Transaction |
|---|---|---|
| G-account | open, `create_and_fill_with_fee` | 80aea210de8a2ffb4e7880b2ccf58d72ead16d742ef9f67163f30b9666d46eb6 |
| Smart account (canonical wasm, ed25519 signer) | open, `create_and_fill_with_fee` | 555a47af9b0aab87b1342374223a8b63c24f0303533ea367163274aa7c3f0e10 |
| G-account | close, `create_and_try_fill_with_fee` | af8a141c9e9fe806649d166d8e5a412860ed9036107fba0693177bd31be19d86 |
| Smart account | close, `create_and_fill_with_fee` | d58bf4e9b26136e5c2b2269aedcca4d63ea5e8cebc95e7eb2edb4c468b95550e |

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
| fee-forwarder | CDRHA53H3U35NVQ3PHTUONMG7NSQFFRCQL5QBGTFGTZE5I2735KDLOHR | 6d4c37912a24a2f17919a122a302538b8e3010d5e7ffff40ade9e359a1edf956 |

The testnet contracts were built from the legacy local sources
(`soroban-smart-account/session-policy` @ 9cded45 and `soroban-referral`), which reproduce the
deployed wasm byte-for-byte with rustc 1.93.1, stellar CLI 25.2.0 and their original lockfiles.
This workspace builds with soroban-sdk 26 and one shared lockfile, so its output does not match
those hashes. The referral functions and their argument types are unchanged; session-policy v2
changes `SessionConfig` and adds `get_session`.
