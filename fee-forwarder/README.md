# fee-forwarder

Collects a relay fee in a token, then calls the target. Stateless.

```
forward(fee_token, fee_amount, max_fee_amount, expiration_ledger,
        target_contract, target_fn, target_args, user, fee_recipient) -> Val
forward_dynamic(<same arguments>) -> Val
```

`user` signs `(fee_token, max_fee_amount, expiration_ledger, fee_recipient, target_contract,
target_fn, target_args)`. `forward_dynamic` signs the same without `target_args`. The relayer sets
`fee_amount`, above zero and at most `max_fee_amount`.

The fee is collected with OpenZeppelin's `collect_fee` (Eager): the full `max_fee_amount` is
pulled, `fee_amount` goes to `fee_recipient`, the rest is refunded.

| Error | |
|---|---|
| 6001 | `target_fn` is `transfer_from` or `burn_from` |
| 6002 | `fee_recipient` is the forwarder |
| 5000–5006 | OpenZeppelin fee errors |

Event: `fee_collected`, topics `[user, recipient]`, data `{amount, token}`.
