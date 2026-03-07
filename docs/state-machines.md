# State Machines

This document reflects the current live runtime after copy trading was removed.

## 1. Arbitrage Execution Lifecycle

```text
arb-monitor
  -> Redis forwarder
  -> arb_executor
       validate freshness / min profit / depth / circuit breaker / dedup
       -> create PENDING position
       -> buy YES
       -> buy NO
       -> mark OPEN
  -> exit_handler
       hold-to-resolution positions:
         - poll market resolution
         - close via resolution
       exit-on-correction positions:
         - evaluate mark-to-market bids
         - mark EXIT_READY
         - execute sell orders
         - close via exit
```

Position states:
- `Pending`: entry submitted, waiting for both legs
- `Open`: live position
- `ExitReady`: eligible for market exit
- `Closing`: exit orders in flight
- `Closed`: realized P&L persisted
- `EntryFailed` / `ExitFailed` / `Stalled`: recovery states

Important ownership:
- `arb_executor` owns entry only.
- `exit_handler` owns close readiness, exit execution, and resolution close.
- `circuit_breaker` should receive realized exit P&L, not estimated entry P&L.

## 2. Quant Signal Lifecycle

```text
feature calculators / aggregates
  -> signal generators
       flow
       mean_reversion
       resolution_proximity
       cross_market
  -> quant_signal_executor
       persist signal
       validate enabled / age / expiry / confidence
       validate dedup / circuit breaker / per-strategy halt
       validate token lookup / depth / max positions
       -> create PENDING position
       -> buy YES or NO single leg
       -> mark OPEN
       -> link quant_signals.position_id
  -> exit_handler
       evaluate open ExitOnCorrection positions
       update unrealized P&L from current bids
       mark EXIT_READY on generic TP / SL / max-hold rules
       execute sell on held leg(s)
       close position
  -> quant executor outcome sync
       fold realized quant P&L into in-memory per-strategy breaker state
```

Current close policy for quant:
- take profit threshold
- stop loss threshold
- max hold duration

Current limitation:
- exit logic is generic across strategies; there is not yet a strategy-specific close model.

## 3. Circuit Breaker

```text
NORMAL
  -> record realized trade P&L
  -> update daily_pnl / consecutive_losses / current_value / peak_value
  -> if thresholds breached:
       DailyLossLimit | MaxDrawdown | ConsecutiveLosses
       -> TRIPPED
TRIPPED
  -> can_trade = false
  -> manual reset / cooldown / recovery mode transitions
```

Notes:
- Drawdown uses portfolio value state.
- Daily P&L resets at UTC midnight.
- Recovery mode applies stricter scaled thresholds.

## 4. Dynamic Tuning

```text
runtime metrics + DB snapshots + circuit breaker + regime
  -> dynamic_tuner
       seed defaults
       collect execution metrics
       compute bounded targets
       write dynamic_config
       publish Redis updates
  -> runtime subscriber
       apply supported knobs live
```

Currently applied live knobs:
- `ARB_POSITION_SIZE`
- `ARB_MIN_NET_PROFIT`
- `ARB_MIN_BOOK_DEPTH`
- `ARB_MAX_SIGNAL_AGE_SECS`
- `QUANT_BASE_POSITION_SIZE`

Current emphasis:
- arb tuning is the main mature path
- quant tuning is limited mostly to base-size adaptation

## 5. Strategy Performance Snapshots

```text
closed positions + quant_signals
  -> strategy_pnl_calculator
       aggregate 7d / 30d snapshots
       upsert strategy_pnl_snapshots
  -> dashboard / signals API
```

Sources:
- `arb` from `positions.source = 1`
- quant strategies from `quant_signals.kind` joined to closed positions with `positions.source = 3`

## 6. Wallet Research Flow

```text
wallet harvester + wallet_trades
  -> metrics_calculator
  -> wallet_success_metrics
  -> wallet discovery / scoring / prediction
  -> advisory recommendation endpoints
```

Important boundary:
- wallet discovery and recommendations are research/operator tooling.
- they do not currently drive live copy trading or automated wallet rotation.
