# market-router

Stateless call router for the Zenex markets.

| Function | |
|---|---|
| `multicall(calls)` | Runs every call; any failure reverts all. |
| `multicall_try(calls)` | Runs every call; a failed call returns its error in its slot. |
| `create_and_fill(calls, user, keeper, price)` | Runs `calls`, then fills the order `calls[0]` created. A failed fill reverts all. |
| `create_and_try_fill(calls, user, keeper, price)` | Same, but a failed fill leaves the orders open and returns its error. |

`Call` is `{ contract, func, args }`. The fill functions append the fill result to the call results.
