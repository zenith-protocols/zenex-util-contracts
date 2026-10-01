# wallet-factory

Deploys the pinned smart-account wasm. No admin.

Constructor: `(account_wasm_hash)`.

```
deploy(signers, policies) -> Address
predict_address(signers, policies) -> Address
```

The salt is the SHA-256 of `(signers, policies)` as XDR, so the address commits to the wallet's
signers and anyone may deploy it. Deploying the same arguments twice fails.
