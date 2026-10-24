# Polygon fork checks

The bot's on-chain money paths, run for real against a local fork of
Polygon mainnet. The deployed Polymarket and token contracts execute every
transaction, and nothing reaches the real chain. Run these before turning
on live trading, and after any change to withdrawals, approvals or the
collateral sweeper.

## Start the fork

```bash
cd scripts/polygon-fork
npm install
npm run relay &          # retries the upstream node's transient refusals
HEAD=$(curl -s -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"eth_blockNumber","params":[]}' \
  http://127.0.0.1:8546 | python3 -c 'import json,sys; print(int(json.load(sys.stdin)["result"], 16))')
FORK_BLOCK=$((HEAD - 3000)) FORK_URL=http://127.0.0.1:8546 npm run fork
```

The upstream node (`UPSTREAM`, drpc by default) must serve historical
state. The fork is pinned a few thousand blocks back because load-balanced
providers route some requests to backends that lag the head.

## Core money paths

```bash
FORK_RPC_URL=http://127.0.0.1:8545 \
  cargo test -p polymarket-core --test fork_money_paths -- --ignored --test-threads=1
```

This runs the bot's code on its own, with no API server. It checks:

- The start-up approvals: all eight are set, and none are repeated on a
  second run.
- The collateral sweeper's wrap of USDC.e into pUSD.
- A USDC.e withdrawal, and a pUSD withdrawal through the Offramp.
- An unfunded withdrawal is refused before anything is signed.
- Sending a mined withdrawal again is never read as refused.
- A fee-bump replacement pays out once.

## Withdrawals through the API

`withdrawal_e2e.sh` drives the real API server, its journal and its
reconciler. Use a fresh database with a seeded admin, and start the server
against the fork with `POLYGON_RPC_URL` and `WITHDRAWAL_VERIFY_RPC_URL` set
to the fork, `WITHDRAWAL_DESTINATION_DELAY_HOURS=0` and
`WITHDRAWAL_DAILY_CAP_USD=50`. The script's header lists every variable.

It checks:

- A destination not on the allowlist is refused.
- The preflight caps the amount at the daily limit.
- Repeating a request returns the same withdrawal.
- A withdrawal over the daily limit is refused.
- USDC.e and pUSD payouts each arrive once and are confirmed.
- A pUSD withdrawal without the Offramp approval is refused, and the message
  names the approval.
- A stuck withdrawal gets a gas bump, and it pays out once whichever of the
  two transactions is mined.

It takes about seven minutes, because the reconciler runs every 30 seconds.
