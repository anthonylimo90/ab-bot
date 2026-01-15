# State Machine Diagrams

This document describes the state machines and workflows in the Polymarket Scanner System.

## Table of Contents

1. [Position Lifecycle](#1-position-lifecycle)
2. [Order Execution](#2-order-execution)
3. [Circuit Breaker](#3-circuit-breaker)
4. [Stop-Loss Management](#4-stop-loss-management)
5. [Copy Trading Workflow](#5-copy-trading-workflow)
6. [Wallet Discovery & Prediction](#6-wallet-discovery--prediction)
7. [Bot Detection Scoring](#7-bot-detection-scoring)
8. [Backtest Simulation](#8-backtest-simulation)
9. [Cross-System Integration](#9-cross-system-integration)

---

## 1. Position Lifecycle

The core state machine for arbitrage position tracking.

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                         POSITION LIFECYCLE                                   │
└─────────────────────────────────────────────────────────────────────────────┘

                              ┌──────────────────┐
                              │                  │
        Arb Signal Detected   │     PENDING      │
        ───────────────────►  │                  │
                              │  Entry awaiting  │
                              │   execution      │
                              └────────┬─────────┘
                                       │
                                       │ mark_open()
                                       │ [Both YES + NO purchased]
                                       ▼
                              ┌──────────────────┐
                              │                  │
                              │      OPEN        │◄─────────────────┐
                              │                  │                  │
                              │  Monitoring P&L  │   update_pnl()   │
                              │  & spread        │   [price change] │
                              └────────┬─────────┘─────────────────►┘
                                       │
                                       │ mark_exit_ready()
                                       │ [Spread normalized OR
                                       │  stop-loss triggered]
                                       ▼
                              ┌──────────────────┐
                              │                  │
                              │   EXIT_READY     │
                              │                  │
                              │  Profitable exit │
                              │  available       │
                              └────────┬─────────┘
                                       │
                                       │ mark_closing()
                                       │ [Exit initiated]
                                       ▼
                              ┌──────────────────┐
                              │                  │
                              │    CLOSING       │
                              │                  │
                              │  Awaiting exit   │
                              │  confirmation    │
                              └────────┬─────────┘
                                       │
               ┌───────────────────────┼───────────────────────┐
               │                       │                       │
               │ close_via_exit()      │                       │ close_via_resolution()
               │ [Market exit]         │                       │ [Market resolves]
               ▼                       │                       ▼
      ┌────────────────┐               │              ┌────────────────┐
      │                │               │              │                │
      │ CLOSED (Exit)  │               │              │ CLOSED (Res.)  │
      │                │               │              │                │
      │ exit_yes_price │               │              │ resolution=    │
      │ exit_no_price  │               │              │ YES/NO         │
      └────────────────┘               │              └────────────────┘
                                       │
                                       │
        ───────────────────────────────┴───────────────────────────────
        Note: close_via_resolution() can be called from ANY active state
              (Pending, Open, ExitReady, Closing)

States:
  - Pending:    Entry signal detected, awaiting order execution
  - Open:       Both sides purchased, actively monitoring spread
  - ExitReady:  Exit opportunity available (spread corrected)
  - Closing:    Exit orders submitted, awaiting fills
  - Closed:     Position fully closed with final P&L

Files: crates/polymarket-core/src/types/position.rs
       crates/arb-monitor/src/position_tracker.rs
```

---

## 2. Order Execution

State machine for individual order lifecycle.

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                           ORDER EXECUTION                                    │
└─────────────────────────────────────────────────────────────────────────────┘

                              ┌──────────────────┐
                              │                  │
         Order Created        │     CREATED      │
         ────────────────►    │                  │
                              │  Order built,    │
                              │  not submitted   │
                              └────────┬─────────┘
                                       │
                                       │ submit_order()
                                       │ [Send to exchange]
                                       ▼
                              ┌──────────────────┐
                              │                  │
                              │     PENDING      │
                              │                  │
                              │  Submitted to    │
                              │  exchange        │
                              └────────┬─────────┘
                                       │
           ┌───────────────────────────┼───────────────────────────┐
           │                           │                           │
           │ cancel_order()            │ partial_fill()            │ reject()
           │                           │                           │
           ▼                           ▼                           ▼
  ┌────────────────┐         ┌──────────────────┐        ┌────────────────┐
  │                │         │                  │        │                │
  │   CANCELLED    │         │ PARTIALLY_FILLED │        │    REJECTED    │
  │                │         │                  │        │                │
  │  User action   │         │  Some quantity   │        │  Exchange      │
  └────────────────┘         │  executed        │        │  denied order  │
                             └────────┬─────────┘        └────────────────┘
                                      │
                  ┌───────────────────┼───────────────────┐
                  │                   │                   │
                  │ fill_remaining()  │ cancel()          │ expire()
                  │                   │                   │
                  ▼                   ▼                   ▼
         ┌────────────────┐  ┌────────────────┐  ┌────────────────┐
         │                │  │                │  │                │
         │     FILLED     │  │   CANCELLED    │  │    EXPIRED     │
         │                │  │                │  │                │
         │  Fully         │  │  Partial fill  │  │  Time limit    │
         │  executed      │  │  + cancel      │  │  reached       │
         └────────────────┘  └────────────────┘  └────────────────┘


Order Types:
  ┌─────────────────┐   ┌─────────────────┐   ┌─────────────────┐
  │   MarketOrder   │   │   LimitOrder    │   │    ArbOrder     │
  ├─────────────────┤   ├─────────────────┤   ├─────────────────┤
  │ - token_id      │   │ - token_id      │   │ - market_id     │
  │ - side (Buy/    │   │ - side          │   │ - yes_order     │
  │   Sell)         │   │ - price         │   │ - no_order      │
  │ - quantity      │   │ - quantity      │   │ - combined_cost │
  │ - market_id     │   │ - order_type    │   │ - expected_     │
  │ - urgency       │   │   (GTC/FOK/IOC) │   │   profit        │
  └─────────────────┘   └─────────────────┘   └─────────────────┘

Files: crates/polymarket-core/src/types/order.rs
       crates/trading-engine/src/executor.rs
```

---

## 3. Circuit Breaker

Risk management state machine for system-wide trading halts.

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                           CIRCUIT BREAKER                                    │
└─────────────────────────────────────────────────────────────────────────────┘

                    ┌──────────────────────────────────────────┐
                    │                                          │
                    │               NORMAL                     │
                    │            (can_trade: true)             │
                    │                                          │
                    │  - Recording trades                      │
                    │  - Monitoring loss/drawdown              │
                    │  - Counting consecutive losses           │
                    │                                          │
                    └─────────────────┬────────────────────────┘
                                      │
    ┌─────────────────────────────────┼─────────────────────────────────┐
    │                                 │                                 │
    │ Daily Loss > $1000              │ Drawdown > 10%                  │ 5+ Consecutive
    │ ─────────────────               │ ────────────                    │ Losses
    │                                 │                                 │ ───────────────
    ▼                                 ▼                                 ▼
┌─────────┐                     ┌─────────┐                       ┌─────────┐
│ TRIPPED │                     │ TRIPPED │                       │ TRIPPED │
│ Daily   │                     │ Drawdown│                       │ Consec. │
│ Loss    │                     │         │                       │ Losses  │
└─────────┘                     └─────────┘                       └─────────┘
    │                                 │                                 │
    └─────────────────────────────────┼─────────────────────────────────┘
                                      │
                                      ▼
                    ┌──────────────────────────────────────────┐
                    │                                          │
                    │              TRIPPED                      │
                    │           (can_trade: false)             │
                    │                                          │
                    │  - Trading halted                        │
                    │  - Cooldown period active                │
                    │  - All orders blocked                    │
                    │                                          │
                    │  trip_reason: TripReason enum            │
                    │  tripped_at: DateTime                    │
                    │                                          │
                    └─────────────────┬────────────────────────┘
                                      │
            ┌─────────────────────────┼─────────────────────────┐
            │                         │                         │
            │ reset()                 │ reset_daily()           │ Cooldown expires
            │ [Manual]                │ [New trading day]       │ (60 min default)
            │                         │                         │
            ▼                         ▼                         ▼
                    ┌──────────────────────────────────────────┐
                    │               NORMAL                      │
                    └──────────────────────────────────────────┘


Trip Reasons:
  ┌────────────────────┐
  │    TripReason      │
  ├────────────────────┤
  │ - DailyLossLimit   │  Absolute daily loss exceeded
  │ - MaxDrawdown      │  Peak-to-trough drawdown exceeded
  │ - ConsecutiveLosses│  Too many losses in a row
  │ - Manual           │  Administrator triggered
  │ - Connectivity     │  API/exchange connection issues
  │ - MarketConditions │  Unusual volatility/liquidity
  └────────────────────┘

Configuration:
  max_daily_loss:        $1,000 (default)
  max_drawdown_pct:      10% (default)
  max_consecutive_losses: 5 (default)
  cooldown_minutes:      60 (default)

Files: crates/risk-manager/src/circuit_breaker.rs
```

---

## 4. Stop-Loss Management

Multi-type stop-loss workflow with advanced compound conditions.

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                         STOP-LOSS MANAGEMENT                                 │
└─────────────────────────────────────────────────────────────────────────────┘

                             ┌──────────────────┐
                             │                  │
      Create Rule            │    INACTIVE      │
      ───────────────►       │                  │
                             │  Rule defined    │
                             │  but not active  │
                             └────────┬─────────┘
                                      │
                                      │ activate()
                                      │ [Position opened]
                                      ▼
                             ┌──────────────────┐
                             │                  │◄──────────────────┐
                             │    ACTIVATED     │                   │
                             │   (MONITORING)   │   update_peak()   │
                             │                  │   [Price rises,   │
                             │  Checking price  │    trailing only] │
                             │  each tick       │───────────────────┘
                             └────────┬─────────┘
                                      │
                                      │ is_triggered() = true
                                      │ [Stop condition met]
                                      ▼
                             ┌──────────────────┐
                             │                  │
                             │    TRIGGERED     │
                             │                  │
                             │  Exit signal     │
                             │  generated       │
                             └────────┬─────────┘
                                      │
              ┌───────────────────────┼───────────────────────┐
              │                       │                       │
              │ execute_stop()        │ manual_exit()         │ cancel_rule()
              │ [Auto execution]      │ [User action]         │
              ▼                       ▼                       ▼
     ┌────────────────┐      ┌────────────────┐      ┌────────────────┐
     │                │      │                │      │                │
     │    EXECUTED    │      │    EXECUTED    │      │   CANCELLED    │
     │   (Auto)       │      │   (Manual)     │      │                │
     │                │      │                │      │  Rule removed  │
     └────────────────┘      └────────────────┘      └────────────────┘


Stop Types:
  ┌─────────────────────────────────────────────────────────────────────────┐
  │                                                                         │
  │   FIXED                PERCENTAGE            TRAILING                   │
  │   ─────                ──────────            ────────                   │
  │   trigger_price        loss_pct              offset_pct + peak_price    │
  │                                                                         │
  │   Price < trigger      Loss > X% of         Price < peak * (1 - offset) │
  │                        entry price                                      │
  │                                                                         │
  │   TIME_BASED                                                            │
  │   ──────────                                                            │
  │   deadline: DateTime                                                    │
  │                                                                         │
  │   Now >= deadline                                                       │
  │                                                                         │
  └─────────────────────────────────────────────────────────────────────────┘


Advanced Compound Stops:
  ┌─────────────────────────────────────────────────────────────────────────┐
  │                       COMPOUND STOP LOGIC                               │
  │                                                                         │
  │   CompoundLogic::And      All conditions must be true                   │
  │   CompoundLogic::Or       Any condition triggers                        │
  │   CompoundLogic::AtLeast  At least N conditions true                    │
  │                                                                         │
  │   StopCondition variants:                                               │
  │   ┌─────────────────────┬────────────────────────────────────────────┐  │
  │   │ PriceBelow          │ Current price below threshold              │  │
  │   │ PercentageFromPeak  │ Fallen X% from highest price               │  │
  │   │ LossExceeds         │ Absolute loss exceeds amount               │  │
  │   │ TimeReached         │ Deadline passed                            │  │
  │   │ VolatilityExceeds   │ Market volatility too high                 │  │
  │   │ VolumeBelowAvg      │ Trading volume dropped                     │  │
  │   │ OutsideMarketHours  │ Outside prime trading hours                │  │
  │   │ ConsecutiveDownCndl │ X consecutive red candles                  │  │
  │   │ SupportBroken       │ Price broke support level                  │  │
  │   └─────────────────────┴────────────────────────────────────────────┘  │
  │                                                                         │
  │   Example: Compound(Or, [PriceBelow(0.50), TimeReached(deadline)])      │
  │            → Triggers if price < $0.50 OR deadline reached              │
  │                                                                         │
  └─────────────────────────────────────────────────────────────────────────┘

Files: crates/risk-manager/src/stop_loss.rs
       crates/risk-manager/src/advanced_stops.rs
```

---

## 5. Copy Trading Workflow

Workflow for tracking and mirroring wallet trades.

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                         COPY TRADING WORKFLOW                                │
└─────────────────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────────────────┐
│                           WALLET TRACKING                                    │
└─────────────────────────────────────────────────────────────────────────────┘

              add_tracked_wallet()
              ────────────────────►  ┌──────────────────┐
                                     │                  │
                                     │    TRACKING      │◄────────────────┐
                                     │    (Enabled)     │                 │
                                     │                  │  set_wallet_    │
                                     │  Monitoring for  │  enabled(true)  │
                                     │  new trades      │                 │
                                     └────────┬─────────┘                 │
                                              │                           │
                  ┌───────────────────────────┼───────────────────────────┤
                  │                           │                           │
                  │ set_wallet_enabled(false) │ remove_tracked_wallet()   │
                  │                           │                           │
                  ▼                           ▼                           │
         ┌────────────────┐          ┌────────────────┐                   │
         │                │          │                │                   │
         │    DISABLED    │          │    REMOVED     │                   │
         │                │──────────│                │                   │
         │  Paused, not   │          │  Wallet no     │                   │
         │  copying       │          │  longer tracked│                   │
         └────────────────┘          └────────────────┘                   │
                  │                                                       │
                  └───────────────────────────────────────────────────────┘


┌─────────────────────────────────────────────────────────────────────────────┐
│                          COPY EXECUTION FLOW                                 │
└─────────────────────────────────────────────────────────────────────────────┘

  ┌──────────────┐
  │ Trade Signal │   TradeMonitor detects wallet trade
  │   Detected   │
  └──────┬───────┘
         │
         ▼
  ┌──────────────┐
  │ Wallet       │   Check: wallet.enabled == true?
  │ Enabled?     │──────No────► [Ignore signal]
  └──────┬───────┘
         │ Yes
         ▼
  ┌──────────────┐
  │ Apply Copy   │   Wait for copy_delay_ms (default: 0)
  │ Delay        │   (Avoids front-running detection)
  └──────┬───────┘
         │
         ▼
  ┌──────────────┐
  │ Calculate    │   Based on AllocationStrategy:
  │ Allocation   │   - EqualWeight: 1/N of capital
  └──────┬───────┘   - ConfiguredWeight: wallet.allocation_pct
         │           - PerformanceWeighted: by historical ROI
         ▼           - RiskAdjusted: by Sharpe ratio
  ┌──────────────┐
  │ Determine    │   qty = min(calculated_qty, max_position_size)
  │ Copy Qty     │   Respects wallet.max_position_size limit
  └──────┬───────┘
         │
         ▼
  ┌──────────────┐
  │ Execute      │   OrderExecutor.execute_market_order()
  │ Order        │   Paper mode or Live mode
  └──────┬───────┘
         │
         ▼
  ┌──────────────┐
  │ Update       │   wallet.last_copied_trade = now
  │ Metrics      │   wallet.total_copied_value += value
  └──────────────┘


Allocation Strategies:
  ┌───────────────────────────────────────────────────────────────────────┐
  │                                                                       │
  │   EqualWeight          All wallets get equal capital                  │
  │   ───────────          allocation = total_capital / num_wallets       │
  │                                                                       │
  │   ConfiguredWeight     Use preconfigured percentages                  │
  │   ────────────────     allocation = total_capital * wallet.alloc_pct  │
  │                                                                       │
  │   PerformanceWeighted  Weight by historical returns                   │
  │   ───────────────────  allocation ∝ wallet.roi                        │
  │                                                                       │
  │   RiskAdjusted         Weight by risk-adjusted returns                │
  │   ────────────         allocation ∝ wallet.sharpe_ratio               │
  │                                                                       │
  └───────────────────────────────────────────────────────────────────────┘

Files: crates/trading-engine/src/copy_trader.rs
       crates/wallet-tracker/src/trade_monitor.rs
```

---

## 6. Wallet Discovery & Prediction

Workflow for finding profitable wallets and predicting success.

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                     WALLET DISCOVERY & PREDICTION                            │
└─────────────────────────────────────────────────────────────────────────────┘


┌─────────────────────────────────────────────────────────────────────────────┐
│                         DISCOVERY WORKFLOW                                   │
└─────────────────────────────────────────────────────────────────────────────┘

  ┌──────────────┐
  │ Define       │   DiscoveryCriteria::builder()
  │ Criteria     │     .min_trades(50)
  └──────┬───────┘     .min_win_rate(0.6)
         │             .min_volume(10000)
         ▼             .time_window_days(30)
  ┌──────────────┐     .exclude_bots(true)
  │ Query        │
  │ Database     │   SELECT wallets matching criteria
  └──────┬───────┘
         │
         ▼
  ┌──────────────┐
  │ Calculate    │   For each wallet:
  │ Metrics      │   - ROI, Sharpe, Sortino
  └──────┬───────┘   - Win rate, max drawdown
         │           - Trade count, consistency
         ▼
  ┌──────────────┐
  │ Filter       │   Apply min thresholds
  │ Results      │   Remove bot-flagged wallets
  └──────┬───────┘
         │
         ▼
  ┌──────────────┐
  │ Rank         │   Sort by RankingMetric:
  │ Wallets      │   - ROI, WinRate, Volume
  └──────┬───────┘   - PnL, TradeCount, Consistency
         │
         ▼
  ┌──────────────┐
  │ Return       │   Vec<DiscoveredWallet>
  │ Results      │
  └──────────────┘


┌─────────────────────────────────────────────────────────────────────────────┐
│                        PREDICTION WORKFLOW                                   │
└─────────────────────────────────────────────────────────────────────────────┘

  ┌──────────────┐
  │ Input        │   Historical wallet metrics
  │ Metrics      │   from ProfitabilityAnalyzer
  └──────┬───────┘
         │
         ▼
  ┌──────────────┐
  │ Select       │   PredictionModel:
  │ Model        │   - RuleBased (heuristics)
  └──────┬───────┘   - Linear (weighted sum)
         │           - WeightedAverage
         ▼
  ┌──────────────┐
  │ Calculate    │   Factor weights:
  │ Factors      │   ┌────────────────────────┐
  └──────┬───────┘   │ win_rate:      0.25    │
         │           │ sharpe_ratio:  0.20    │
         │           │ consistency:   0.20    │
         │           │ roi:           0.15    │
         │           │ drawdown:      0.10    │
         │           │ trade_count:   0.05    │
         │           │ recency:       0.05    │
         ▼           └────────────────────────┘
  ┌──────────────┐
  │ Compute      │   probability = Σ(factor * weight)
  │ Score        │   confidence = f(data_quality, sample_size)
  └──────┬───────┘
         │
         ▼
  ┌──────────────┐
  │ Categorize   │
  │              │
  └──────────────┘


Prediction Categories:
  ┌───────────────────────────────────────────────────────────────────────┐
  │                                                                       │
  │   probability >= 0.70  ───────►  HighPotential   (Strong performer)   │
  │                                                                       │
  │   probability >= 0.50  ───────►  Moderate        (Decent performer)   │
  │                                                                       │
  │   probability < 0.50   ───────►  LowPotential    (Risky/weak)         │
  │                                                                       │
  │   confidence < 0.30    ───────►  Uncertain       (Insufficient data)  │
  │                                                                       │
  └───────────────────────────────────────────────────────────────────────┘

Files: crates/wallet-tracker/src/discovery.rs
       crates/wallet-tracker/src/profitability.rs
       crates/wallet-tracker/src/success_predictor.rs
```

---

## 7. Bot Detection Scoring

Flow for analyzing wallet behavior and classifying as human or bot.

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                          BOT DETECTION SCORING                               │
└─────────────────────────────────────────────────────────────────────────────┘

  ┌──────────────┐
  │ Wallet       │   Input: wallet address + trade history
  │ Address      │
  └──────┬───────┘
         │
         ▼
  ┌──────────────────────────────────────────────────────────────────────────┐
  │                      FEATURE EXTRACTION                                   │
  │                                                                           │
  │   ┌───────────────────────────────────────────────────────────────────┐  │
  │   │ 1. Trade Interval Analysis                                        │  │
  │   │    Calculate coefficient of variation (CV) of trade intervals     │  │
  │   │    CV < 0.1 indicates highly regular/automated timing             │  │
  │   └───────────────────────────────────────────────────────────────────┘  │
  │                                                                           │
  │   ┌───────────────────────────────────────────────────────────────────┐  │
  │   │ 2. Win Rate Analysis                                              │  │
  │   │    win_rate > 90% with 100+ trades is suspicious                  │  │
  │   │    (Humans rarely achieve this consistently)                      │  │
  │   └───────────────────────────────────────────────────────────────────┘  │
  │                                                                           │
  │   ┌───────────────────────────────────────────────────────────────────┐  │
  │   │ 3. Opposing Position Detection                                    │  │
  │   │    Holding both YES and NO = arbitrage bot signature              │  │
  │   └───────────────────────────────────────────────────────────────────┘  │
  │                                                                           │
  │   ┌───────────────────────────────────────────────────────────────────┐  │
  │   │ 4. Reaction Latency                                               │  │
  │   │    avg_latency < 500ms suggests automated execution               │  │
  │   └───────────────────────────────────────────────────────────────────┘  │
  │                                                                           │
  │   ┌───────────────────────────────────────────────────────────────────┐  │
  │   │ 5. Activity Pattern                                               │  │
  │   │    Active in all 24 hours = likely automated                      │  │
  │   └───────────────────────────────────────────────────────────────────┘  │
  │                                                                           │
  └──────────────────────────────────────────────────────────────────────────┘
         │
         ▼
  ┌──────────────────────────────────────────────────────────────────────────┐
  │                         SCORING                                           │
  │                                                                           │
  │   Signal                  Condition                   Points              │
  │   ─────────────────────────────────────────────────────────────           │
  │   ConsistentIntervals     CV < 0.10                   +30                 │
  │   HighWinRate             >90% with 100+ trades       +25                 │
  │   OpposingPositions       Has YES+NO in same market   +20                 │
  │   FastLatency             avg < 500ms                 +15                 │
  │   AlwaysActive            Active all 24 hours         +10                 │
  │   ─────────────────────────────────────────────────────────────           │
  │   Maximum possible score:                             100                 │
  │                                                                           │
  └──────────────────────────────────────────────────────────────────────────┘
         │
         ▼
  ┌──────────────────────────────────────────────────────────────────────────┐
  │                       CLASSIFICATION                                      │
  │                                                                           │
  │        Score < 25           25 ≤ Score < 50          Score ≥ 50          │
  │            │                      │                       │               │
  │            ▼                      ▼                       ▼               │
  │   ┌────────────────┐    ┌────────────────┐     ┌────────────────┐        │
  │   │  LIKELY_HUMAN  │    │   SUSPICIOUS   │     │   LIKELY_BOT   │        │
  │   │                │    │                │     │                │        │
  │   │ Safe to copy   │    │ Needs review   │     │ Exclude from   │        │
  │   │                │    │                │     │ copy trading   │        │
  │   └────────────────┘    └────────────────┘     └────────────────┘        │
  │                                                                           │
  └──────────────────────────────────────────────────────────────────────────┘


Output:
  ┌───────────────────────────────────────────────────────────────────────┐
  │   WalletAnalysis {                                                    │
  │     address: "0x...",                                                 │
  │     score: 45,                                                        │
  │     classification: Suspicious,                                       │
  │     signals: [                                                        │
  │       ConsistentIntervals { cv: 0.08, points: 30 },                   │
  │       FastLatency { avg_ms: 350.0, points: 15 }                       │
  │     ],                                                                │
  │     analyzed_at: "2026-01-10T..."                                     │
  │   }                                                                   │
  └───────────────────────────────────────────────────────────────────────┘

Files: crates/polymarket-core/src/types/wallet.rs
       crates/bot-scanner/src/
```

---

## 8. Backtest Simulation

Simulation engine for testing strategies against historical data.

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                         BACKTEST SIMULATION                                  │
└─────────────────────────────────────────────────────────────────────────────┘

  ┌──────────────┐
  │ Configure    │   SimulatorConfig {
  │ Simulator    │     initial_capital,
  └──────┬───────┘     commission_rate,
         │             slippage_model,
         │             max_position_pct,
         │             allow_short,
         ▼             reinvest_profits
  ┌──────────────┐   }
  │ Load         │
  │ Strategy     │   impl Strategy for MyStrategy
  └──────┬───────┘
         │
         ▼
  ┌──────────────┐
  │ Initialize   │   strategy.initialize(&context)
  │ Strategy     │   Set up indicators, state
  └──────┬───────┘
         │
         ▼
  ┌─────────────────────────────────────────────────────────────────────────┐
  │                    SIMULATION LOOP                                       │
  │                                                                          │
  │   for each timestamp in historical_data:                                 │
  │     │                                                                    │
  │     ▼                                                                    │
  │   ┌──────────────┐                                                       │
  │   │ Update       │   Load orderbook snapshot                             │
  │   │ Market Data  │   Update position mark-to-market                      │
  │   └──────┬───────┘                                                       │
  │          │                                                               │
  │          ▼                                                               │
  │   ┌──────────────┐                                                       │
  │   │ on_data()    │   strategy.on_data(&snapshot, &context)               │
  │   │              │   Returns: Vec<Signal>                                │
  │   └──────┬───────┘                                                       │
  │          │                                                               │
  │          ▼                                                               │
  │   ┌──────────────┐                                                       │
  │   │ Process      │   for each signal:                                    │
  │   │ Signals      │     validate_signal()                                 │
  │   └──────┬───────┘     calculate_size()                                  │
  │          │             apply_slippage()                                  │
  │          │             execute_fill()                                    │
  │          ▼                                                               │
  │   ┌──────────────┐                                                       │
  │   │ on_fill()    │   strategy.on_fill(&fill)                             │
  │   │              │   Update strategy state                               │
  │   └──────┬───────┘                                                       │
  │          │                                                               │
  │          ▼                                                               │
  │   ┌──────────────┐                                                       │
  │   │ Record       │   equity_curve.push(portfolio_value)                  │
  │   │ Equity       │   trade_log.push(fill)                                │
  │   └──────────────┘                                                       │
  │                                                                          │
  └─────────────────────────────────────────────────────────────────────────┘
         │
         ▼
  ┌──────────────┐
  │ Finalize     │   strategy.finalize(&context)
  │ Strategy     │   Close remaining positions
  └──────┬───────┘
         │
         ▼
  ┌──────────────┐
  │ Calculate    │   BacktestResult {
  │ Results      │     total_return,
  └──────────────┘     annualized_return,
                       max_drawdown,
                       sharpe_ratio,
                       sortino_ratio,
                       win_rate,
                       profit_factor,
                       total_trades,
                       equity_curve,
                       trade_log
                     }


Built-in Strategies:
  ┌───────────────────────────────────────────────────────────────────────┐
  │                                                                       │
  │   ArbitrageStrategy                                                   │
  │   ─────────────────                                                   │
  │   Entry: yes_ask + no_ask < 0.98 (profitable after 2% fees)           │
  │   Exit:  spread normalizes OR market resolves                         │
  │                                                                       │
  │   MomentumStrategy                                                    │
  │   ────────────────                                                    │
  │   Entry: price > SMA(lookback) by threshold                           │
  │   Exit:  price < SMA(lookback)                                        │
  │                                                                       │
  │   MeanReversionStrategy                                               │
  │   ─────────────────────                                               │
  │   Entry: z-score > threshold (oversold)                               │
  │   Exit:  z-score returns to mean                                      │
  │                                                                       │
  └───────────────────────────────────────────────────────────────────────┘


Slippage Models:
  ┌───────────────────────────────────────────────────────────────────────┐
  │                                                                       │
  │   None              No slippage applied                               │
  │                                                                       │
  │   Fixed(pct)        Constant percentage slippage                      │
  │                     fill_price = price * (1 + pct)                    │
  │                                                                       │
  │   VolumeBased       Slippage increases with order size                │
  │   { base, impact }  slippage = base + (size/volume) * impact          │
  │                                                                       │
  │   SpreadBased       Slippage based on bid-ask spread                  │
  │   { multiplier }    slippage = spread * multiplier                    │
  │                                                                       │
  └───────────────────────────────────────────────────────────────────────┘

Files: crates/backtester/src/simulator.rs
       crates/backtester/src/strategy.rs
       crates/backtester/src/data_store.rs
```

---

## 9. Cross-System Integration

How the state machines interact in the complete system.

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                      COMPLETE SYSTEM FLOW                                    │
└─────────────────────────────────────────────────────────────────────────────┘


                         ┌─────────────────────────────────────┐
                         │        MARKET DATA SOURCES          │
                         │                                     │
                         │  CLOB API        Polygon RPC        │
                         │  (Orderbook)     (On-chain)         │
                         └──────────┬───────────┬──────────────┘
                                    │           │
                    ┌───────────────┴───────────┴───────────────┐
                    │                                           │
                    ▼                                           ▼
         ┌─────────────────────┐                    ┌─────────────────────┐
         │                     │                    │                     │
         │   WALLET TRACKER    │                    │    ARB MONITOR      │
         │                     │                    │                     │
         │  ┌───────────────┐  │                    │  ┌───────────────┐  │
         │  │ TradeMonitor  │  │                    │  │PositionTracker│  │
         │  │ (detect new   │  │                    │  │ (track arb    │  │
         │  │  trades)      │  │                    │  │  positions)   │  │
         │  └───────┬───────┘  │                    │  └───────┬───────┘  │
         │          │          │                    │          │          │
         │  ┌───────▼───────┐  │                    │  ┌───────▼───────┐  │
         │  │ BotScanner    │  │                    │  │ Spread Calc   │  │
         │  │ (filter bots) │  │                    │  │ (entry/exit)  │  │
         │  └───────┬───────┘  │                    │  └───────┬───────┘  │
         │          │          │                    │          │          │
         └──────────┼──────────┘                    └──────────┼──────────┘
                    │                                          │
                    │ WalletTrade                               │ ArbSignal
                    │ Signal                                    │
                    ▼                                          ▼
         ┌─────────────────────────────────────────────────────────────────┐
         │                                                                 │
         │                      TRADING ENGINE                             │
         │                                                                 │
         │   ┌─────────────────┐         ┌─────────────────┐              │
         │   │   CopyTrader    │         │  OrderExecutor  │              │
         │   │                 │────────►│                 │              │
         │   │ - Track wallets │         │ - Paper mode    │              │
         │   │ - Calc allocat. │         │ - Live mode     │              │
         │   │ - Mirror trades │         │ - Sign orders   │              │
         │   └─────────────────┘         └────────┬────────┘              │
         │                                        │                       │
         │   ┌─────────────────┐                  │ ExecutionReport       │
         │   │ PositionManager │◄─────────────────┘                       │
         │   │                 │                                          │
         │   │ - Size limits   │                                          │
         │   │ - Portfolio     │                                          │
         │   └────────┬────────┘                                          │
         │            │                                                   │
         └────────────┼───────────────────────────────────────────────────┘
                      │
                      │ Position Updates
                      ▼
         ┌─────────────────────────────────────────────────────────────────┐
         │                                                                 │
         │                      RISK MANAGER                               │
         │                                                                 │
         │   ┌─────────────────┐         ┌─────────────────┐              │
         │   │ StopLossManager │         │ CircuitBreaker  │              │
         │   │                 │         │                 │              │
         │   │ - Fixed stops   │         │ - Daily limits  │              │
         │   │ - Trailing      │         │ - Drawdown      │              │
         │   │ - Time-based    │         │ - Loss streaks  │              │
         │   │ - Compound      │         │ - Cooldowns     │              │
         │   └────────┬────────┘         └────────┬────────┘              │
         │            │                           │                       │
         │            │ Stop Triggered            │ Circuit Tripped       │
         │            ▼                           ▼                       │
         │        ┌───────────────────────────────────┐                   │
         │        │          HALT TRADING             │                   │
         │        │   (Execute stops / Block orders)  │                   │
         │        └───────────────────────────────────┘                   │
         │                                                                 │
         └─────────────────────────────────────────────────────────────────┘
                      │
                      │ Events
                      ▼
         ┌─────────────────────────────────────────────────────────────────┐
         │                                                                 │
         │                       API SERVER                                │
         │                                                                 │
         │   ┌─────────────────┐         ┌─────────────────┐              │
         │   │    REST API     │         │    WebSocket    │              │
         │   │                 │         │                 │              │
         │   │ /api/v1/...     │         │ /ws/orderbook   │              │
         │   │ - markets       │         │ /ws/positions   │              │
         │   │ - positions     │         │ /ws/signals     │              │
         │   │ - wallets       │         │ /ws/all         │              │
         │   │ - orders        │         │                 │              │
         │   │ - backtest      │         │ Broadcast       │              │
         │   └─────────────────┘         │ real-time       │              │
         │                               │ updates         │              │
         │                               └─────────────────┘              │
         │                                                                 │
         └─────────────────────────────────────────────────────────────────┘
                      │
                      ▼
         ┌─────────────────────────────────────────────────────────────────┐
         │                                                                 │
         │                       DASHBOARD                                 │
         │                                                                 │
         │   ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────┐       │
         │   │  Roster  │  │  Bench   │  │ Rotation │  │ Portfolio│       │
         │   │  (Active │  │ (Discover│  │ (Replace │  │ (Track   │       │
         │   │   5)     │  │  wallets)│  │  wallets)│  │  P&L)    │       │
         │   └──────────┘  └──────────┘  └──────────┘  └──────────┘       │
         │                                                                 │
         │   Demo Mode                    Live Mode                        │
         │   ──────────                   ─────────                        │
         │   Simulated balance            Connected wallet                 │
         │   Paper positions              Real orders                      │
         │                                                                 │
         └─────────────────────────────────────────────────────────────────┘


Data Flow Summary:
  ┌───────────────────────────────────────────────────────────────────────┐
  │                                                                       │
  │   Market Data  ──►  Analysis  ──►  Signals  ──►  Execution            │
  │                                                                       │
  │   CLOB/RPC    Arb Monitor      TradeSignal    OrderExecutor           │
  │               Wallet Tracker   ArbSignal      Paper/Live              │
  │               Bot Scanner      CopySignal                             │
  │                                                                       │
  │                        │                                              │
  │                        ▼                                              │
  │                                                                       │
  │   Positions  ──►  Risk Check  ──►  Protection  ──►  API/Dashboard     │
  │                                                                       │
  │   Position     CircuitBreaker   StopLoss       REST/WebSocket         │
  │   Tracker      Daily limits     Trailing       Real-time updates      │
  │                Drawdown         Compound                              │
  │                                                                       │
  └───────────────────────────────────────────────────────────────────────┘
```

---

## Summary

| State Machine | States | Primary File |
|--------------|--------|--------------|
| Position Lifecycle | Pending → Open → ExitReady → Closing → Closed | `polymarket-core/types/position.rs` |
| Order Execution | Created → Pending → PartiallyFilled/Filled/Cancelled/Rejected/Expired | `polymarket-core/types/order.rs` |
| Circuit Breaker | Normal ⟷ Tripped | `risk-manager/circuit_breaker.rs` |
| Stop-Loss | Inactive → Activated → Triggered → Executed | `risk-manager/stop_loss.rs` |
| Copy Trading | Tracking (Enabled/Disabled) → Removed | `trading-engine/copy_trader.rs` |
| Wallet Discovery | Criteria → Query → Filter → Rank → Results | `wallet-tracker/discovery.rs` |
| Bot Detection | Features → Score → Classify (Human/Suspicious/Bot) | `polymarket-core/types/wallet.rs` |
| Backtest | Initialize → Loop(data→signals→fills) → Finalize → Results | `backtester/simulator.rs` |
