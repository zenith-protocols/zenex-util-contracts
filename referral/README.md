# referral

Records that a wallet was referred by another wallet. No storage, no admin.

```
attribute(caller, referrer)
```

Requires `caller`'s authorization and emits `attributed`, topics `[referee, referrer]`.

| Error | |
|---|---|
| 7001 | `caller` is `referrer` |
