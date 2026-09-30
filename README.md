# zenex-util-contracts

Small standalone Zenex contracts outside the core protocol.

| Contract | Testnet address | Deployed wasm sha256 |
|---|---|---|
| session-policy | CDUUHEXJY3EMQPGGRQS2KVJN7J3RM5HM2QWVUA5AGE3BRUOYW2MZPUAT | a98d1317f918b03af4e23f407eb99711eabda9c64629ae413427e7fa1c4f2135 |
| referral | CAVUAS7CMIXOUXFND77EDNB5OOWBAM4AOAGV4NF6D4JQXAZAAERQDJQQ | 2d459a2180d91b5006ac0154cd97c4f4505165b39971ace0e534c3e549c5dc9d |

Ported from the legacy local sources (`soroban-smart-account/session-policy` @ 9cded45 and
`soroban-referral`), which reproduce the deployed wasm byte-for-byte with rustc 1.93.1,
stellar CLI 25.2.0 and their original lockfiles. This shared workspace resolves one
soroban-sdk version for both, so builds here are not expected to match those hashes.
