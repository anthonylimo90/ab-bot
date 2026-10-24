#!/usr/bin/env bash
# Withdrawals end to end through the API, against a local Polygon fork:
# the destination allowlist, the daily limit, idempotent requests, USDC.e
# and pUSD payouts, and the reconciler's fee bump (with either transaction
# mined). Real contracts run every transfer; nothing reaches the real chain.
#
# Needs:
#   - the fork:   cd scripts/polygon-fork && npm install && npm run fork
#   - a fresh database with an admin:
#       api-server seed-admin --email "$ADMIN_EMAIL" --password "$ADMIN_PASSWORD"
#   - the API server started against both, with
#       POLYGON_RPC_URL=$FORK_RPC_URL WITHDRAWAL_VERIFY_RPC_URL=$FORK_RPC_URL
#       WITHDRAWAL_DESTINATION_DELAY_HOURS=0 WITHDRAWAL_DAILY_CAP_USD=50
#       CORS_ALLOWED_ORIGINS=$ORIGIN, its log written to $API_LOG
#   - psql, curl and python3.
# Run it within a few minutes of starting the fork (see hardhat.config.js).
set -euo pipefail

FORK_RPC_URL=${FORK_RPC_URL:-http://127.0.0.1:8545}
API_URL=${API_URL:?API_URL names the API server}
DATABASE_URL=${DATABASE_URL:?DATABASE_URL names the API server database}
API_LOG=${API_LOG:?API_LOG names the API server log file}
ORIGIN=${ORIGIN:?ORIGIN is one of CORS_ALLOWED_ORIGINS}
ADMIN_EMAIL=${ADMIN_EMAIL:?}
ADMIN_PASSWORD=${ADMIN_PASSWORD:?}

# Hardhat development account 2: unlocked on the fork, public, never funded
# on mainnet.
WALLET=0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC
KEY=0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a
USDC=0x2791Bca1f2de4661ED88A30C99A7a9449Aa84174
PUSD=0xC011a7E12a19f7B1f670d46F03B03f3342E82DFB
CTF=0x4D97DCd97eC945f40cF65F87097ACe5EA0476045
ONRAMP=0x93070a847efEf7F70739046A929D47a521F5B8ee
OFFRAMP=0x2957922Eb93258b93368531d39fAcCA3B4dC5854
SINK=0x00000000000000000000000000000000000c0ffe
DEST=0x$(python3 -c 'import secrets; print(secrets.token_hex(20))')
JAR=$(mktemp)
trap 'rm -f "$JAR"' EXIT

fail() { echo "FAIL: $*" >&2; exit 1; }
pass() { echo "ok - $*"; }
rpc() {
  curl -s -H 'content-type: application/json' \
    -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$1\",\"params\":$2}" "$FORK_RPC_URL"
}
field() { python3 -c "import json,sys; v=json.load(sys.stdin); print(eval(sys.argv[1]))" "$1"; }
word() { printf '%064x' "$1"; }
addr() { printf '000000000000000000000000%s' "${1#0x}"; }
units() { rpc eth_call "[{\"to\":\"$1\",\"data\":\"0x70a08231$(addr "$2")\"},\"latest\"]" | field 'int(v["result"],16)'; }
send() { # from to data
  local out
  out=$(rpc eth_sendTransaction "[{\"from\":\"$1\",\"to\":\"$2\",\"data\":\"$3\"}]")
  echo "$out" | grep -q '"error"' && fail "transaction from $1 to $2: $out"
  return 0
}
# One block at a time: Hardhat keeps no state for the blocks inside a range
# mined at once, and the bot reads balances a few blocks back.
mine() { for _ in $(seq 1 "$1"); do rpc evm_mine '[]' >/dev/null; done; }
fund_usdc() { # to units
  rpc hardhat_impersonateAccount "[\"$CTF\"]" >/dev/null
  rpc hardhat_setBalance "[\"$CTF\",\"0x3635c9adc5dea00000\"]" >/dev/null
  send "$CTF" "$USDC" "0xa9059cbb$(addr "$1")$(word "$2")"
  rpc hardhat_stopImpersonatingAccount "[\"$CTF\"]" >/dev/null
  mine 5
}
api() { # method path [body] -> body, then the status on the last line
  curl -s -b "$JAR" -c "$JAR" -H 'content-type: application/json' \
    -H 'X-Requested-With: ab-bot-dashboard' -H "Origin: $ORIGIN" \
    -w '\n%{http_code}' -X "$1" "$API_URL$2" ${3:+-d "$3"}
}
status_of() { tail -n1 <<<"$1"; }
body_of() { sed '$d' <<<"$1"; }
withdraw() { # amount [request id]
  api POST /api/v1/vault/withdrawals \
    "{\"destination_address\":\"$DEST\",\"amount\":\"$1\",\"client_request_id\":\"${2:-$(python3 -c 'import uuid; print(uuid.uuid4())')}\"}"
}
psql_value() { psql -tA "$DATABASE_URL" -c "$1"; }
settle() { # withdrawal id: mine until the reconciler settles it
  for _ in $(seq 1 40); do
    mine 3
    local s
    s=$(psql_value "SELECT status FROM wallet_withdrawals WHERE id = '$1'")
    case "$s" in confirmed | failed) echo "$s"; return;; esac
    sleep 3
  done
  echo "unsettled"
}
expect_dest() { # whole USDC.e
  local got
  got=$(units "$USDC" "$DEST")
  [ "$got" = "$(($1 * 1000000))" ] || fail "destination holds $got units, expected $1 USDC.e"
}

[ "$(rpc eth_chainId '[]' | field 'v["result"]')" = 0x89 ] || fail "no Polygon fork at $FORK_RPC_URL"
fund_usdc "$WALLET" 100000000

r=$(api POST /api/v1/auth/login "{\"email\":\"$ADMIN_EMAIL\",\"password\":\"$ADMIN_PASSWORD\"}")
[ "$(status_of "$r")" = 200 ] || fail "login: $r"
r=$(api POST /api/v1/admin/workspaces "{\"name\":\"Fork\",\"owner_email\":\"$ADMIN_EMAIL\"}")
[ "$(status_of "$r")" = 201 ] || fail "workspace: $r"
api POST /api/v1/auth/login "{\"email\":\"$ADMIN_EMAIL\",\"password\":\"$ADMIN_PASSWORD\"}" >/dev/null
r=$(api POST /api/v1/vault/wallets "{\"address\":\"$WALLET\",\"private_key\":\"$KEY\",\"label\":\"fork\"}")
[ "$(status_of "$r")" = 201 ] || fail "store wallet: $r"
r=$(api PUT "/api/v1/vault/wallets/$WALLET/primary")
[ "$(status_of "$r")" = 200 ] || fail "primary: $r"

r=$(withdraw 10)
[ "$(status_of "$r")" = 403 ] || fail "withdrawal to an address not on the list: $r"
pass "a destination not on the list is refused"

r=$(api POST /api/v1/vault/withdrawal-destinations "{\"address\":\"$DEST\"}")
[ "$(status_of "$r")" = 201 ] || fail "add destination: $r"
r=$(api GET "/api/v1/vault/withdrawals/preflight?destination_address=$DEST&amount=10")
[ "$(body_of "$r" | field 'v["can_withdraw"] and v["max_withdrawable"]')" = 50 ] \
  || fail "preflight not capped at the daily limit: $r"
pass "preflight allows it, capped at the daily limit"

request=$(python3 -c 'import uuid; print(uuid.uuid4())')
r=$(withdraw 10 "$request")
[ "$(status_of "$r")" = 200 ] || fail "withdrawal: $r"
first=$(body_of "$r" | field 'v["id"]')
r=$(withdraw 10 "$request")
[ "$(body_of "$r" | field 'v["id"]')" = "$first" ] || fail "a repeated request made another withdrawal: $r"
pass "a repeated request returns the same withdrawal"
r=$(withdraw 45)
[ "$(status_of "$r")" = 409 ] || fail "over the daily limit: $r"
pass "a withdrawal over the daily limit is refused"
[ "$(settle "$first")" = confirmed ] || fail "USDC.e withdrawal not confirmed"
expect_dest 10
pass "USDC.e withdrawal paid once and confirmed"

# A wallet holding only pUSD.
send "$WALLET" "$USDC" "0xa9059cbb$(addr $SINK)$(word "$(units "$USDC" "$WALLET")")"
fund_usdc "$WALLET" 30000000
send "$WALLET" "$USDC" "0x095ea7b3$(addr $ONRAMP)$(word 30000000)"
send "$WALLET" "$ONRAMP" "0x62355638$(addr $USDC)$(addr $WALLET)$(word 30000000)"
send "$WALLET" "$PUSD" "0x095ea7b3$(addr $OFFRAMP)$(word 0)"
mine 5
[ "$(units "$PUSD" "$WALLET")" = 30000000 ] && [ "$(units "$USDC" "$WALLET")" = 0 ] \
  || fail "pUSD setup"
r=$(withdraw 20)
[ "$(status_of "$r")" = 409 ] && grep -q Offramp <<<"$r" || fail "pUSD without the Offramp approval: $r"
pass "a pUSD withdrawal without the Offramp approval is refused, naming it"
send "$WALLET" "$PUSD" "0x095ea7b3$(addr $OFFRAMP)ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
mine 5
r=$(withdraw 20)
[ "$(status_of "$r")" = 200 ] || fail "pUSD withdrawal: $r"
[ "$(settle "$(body_of "$r" | field 'v["id"]')")" = confirmed ] || fail "pUSD withdrawal not confirmed"
expect_dest 30
[ "$(units "$PUSD" "$WALLET")" = 10000000 ] || fail "pUSD not spent"
r=$(api GET "/api/v1/vault/withdrawals/preflight?destination_address=$DEST&amount=1")
[ "$(body_of "$r" | field 'v["withdrawn_last_24h"]')" = 30.000000 ] \
  || fail "the refused, unsigned withdrawal counted toward the limit: $r"
pass "pUSD withdrawal paid through the Offramp; the refused one does not count toward the limit"

bump() { # amount, then "replacement" or "original": which one is mined
  local before id original_raw original_hash replacement_hash
  rpc evm_setAutomine '[false]' >/dev/null
  r=$(withdraw "$1")
  [ "$(status_of "$r")" = 200 ] || fail "withdrawal: $r"
  id=$(body_of "$r" | field 'v["id"]')
  original_raw=$(psql_value "SELECT raw_tx FROM wallet_withdrawals WHERE id = '$id'")
  original_hash=$(psql_value "SELECT tx_hash FROM wallet_withdrawals WHERE id = '$id'")
  rpc hardhat_setNextBlockBaseFeePerGas '["0x2e90edd000"]' >/dev/null # 200 gwei
  before=$(grep -c 'Raised the gas price' "$API_LOG" || true)
  # Five minutes pass.
  psql -q "$DATABASE_URL" -c "UPDATE wallet_withdrawals SET signed_at = signed_at - interval '6 minutes', requested_at = requested_at - interval '6 minutes' WHERE id = '$id'"
  for _ in $(seq 1 24); do
    [ "$(grep -c 'Raised the gas price' "$API_LOG" || true)" -gt "$before" ] && break
    sleep 5
  done
  replacement_hash=$(psql_value "SELECT tx_hash FROM wallet_withdrawals WHERE id = '$id'")
  [ "$replacement_hash" != "$original_hash" ] || fail "no replacement signed"
  if [ "$2" = original ]; then
    rpc hardhat_dropTransaction "[\"$replacement_hash\"]" >/dev/null
    rpc hardhat_setNextBlockBaseFeePerGas '["0x3b9aca00"]' >/dev/null
    rpc eth_sendRawTransaction "[\"$original_raw\"]" >/dev/null
  fi
  rpc evm_mine '[]' >/dev/null
  rpc evm_setAutomine '[true]' >/dev/null
  [ "$(settle "$id")" = confirmed ] || fail "bumped withdrawal not confirmed"
  local mined=$original_hash missing=$replacement_hash
  [ "$2" = original ] || { mined=$replacement_hash; missing=$original_hash; }
  [ "$(psql_value "SELECT tx_hash FROM wallet_withdrawals WHERE id = '$id'")" = "$mined" ] \
    || fail "confirmed under a hash that was not mined"
  [ "$(rpc eth_getTransactionReceipt "[\"$missing\"]" | field 'v["result"]')" = None ] \
    || fail "both transactions mined"
}
fund_usdc "$WALLET" 20000000
bump 7 replacement
expect_dest 37
pass "a stuck withdrawal is re-sent with more gas and pays once"
bump 5 original
expect_dest 42
pass "when the original is mined after a fee bump, it settles the withdrawal and pays once"
echo "all withdrawal checks passed"
