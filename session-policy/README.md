# session-policy

OpenZeppelin smart-account policy for a trading session key.

Constructor: `(forwarder, markets, token, fee_recipient)`. Install takes no parameter.

A context is allowed only when the session key signed and it is:

- a forwarder call whose signed `fee_recipient` (argument 3) is `fee_recipient`;
- a `transfer` of `token` into a market, or an `approve` of `token` to the forwarder;
- any call to a market.

Everything else is refused.

| Error | |
|---|---|
| 4002 | Contract not allowed |
| 4003 | Token function not allowed |
| 4004 | Transfer destination is not a market |
| 4005 | Approve spender is not the forwarder |
| 4006 | Wrong fee recipient |
| 4007 | Session key did not sign |
