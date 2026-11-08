# vault

Two small Soroban contracts and a Soroquake fuzzing harness, used to try the
full Soroquake flow end to end.

| Contract | What it does | Status |
| --- | --- | --- |
| `contracts/vault` | Share-based vault: deposit, withdraw, admin yield | Has a planted rounding bug: `withdraw` rounds the payout up |
| `contracts/token` | SEP-41 style token: mint, transfer, approve, transfer_from, burn | Correct |

## Invariants

`invariants/tests/campaign.rs` drives both contracts with random call
sequences from three users and an admin, and checks after every call:

- `share_price_never_drops` (vault): breaks, because of the rounding bug
- `solvent` and `shares_sum_to_total` (vault)
- `supply_matches_balances`, `allowance_never_overspent`, `no_negative_balances` (sep41)

## Run it

```bash
cargo test -p vault -p token                                   # unit tests
SOROQUAKE_TIME=60 cargo test -p invariants --profile fuzz -- --nocapture
SOROQUAKE_SEED=0xfb414568 cargo test -p invariants --profile fuzz -- --nocapture   # replay
```

Set `SOROQUAKE_TOKEN` (and `SOROQUAKE_API` for a self-hosted dashboard) to
stream the campaign to Soroquake. To fix the bug, build the vault with
`--features fixed`.

CI: connect this repository from the Soroquake dashboard; it walks you through
the upload token secret and the workflow file.
