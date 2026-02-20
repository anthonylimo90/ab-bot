'use client';

import { memo } from 'react';
import { Card, CardContent, CardHeader, CardTitle, CardDescription } from '@/components/ui/card';
import { Activity } from 'lucide-react';
import { formatCurrency } from '@/lib/utils';
import type {
  Workspace,
  ServiceStatus,
  RiskStatus,
  DynamicTunerStatus,
} from '@/types/api';

type GateStatus = 'green' | 'amber' | 'red';

interface GateRow {
  label: string;
  status: GateStatus;
  value: string;
}

interface TradingGatesPanelProps {
  workspace: Workspace;
  serviceStatus: ServiceStatus | null;
  riskStatus: RiskStatus | null;
  dynamicTunerStatus: DynamicTunerStatus | null;
}

function statusDot(status: GateStatus) {
  const colors: Record<GateStatus, string> = {
    green: 'bg-green-500',
    amber: 'bg-yellow-500',
    red: 'bg-red-500',
  };
  return (
    <span
      className={`inline-block h-2 w-2 rounded-full shrink-0 ${colors[status]}`}
    />
  );
}

function GateSection({ title, gates }: { title: string; gates: GateRow[] }) {
  return (
    <div>
      <p className="text-xs font-medium text-muted-foreground uppercase tracking-wider mb-2">
        {title}
      </p>
      <div className="space-y-1.5">
        {gates.map((gate) => (
          <div
            key={gate.label}
            className="flex items-center justify-between text-sm gap-2"
          >
            <div className="flex items-center gap-2 min-w-0">
              {statusDot(gate.status)}
              <span className="truncate">{gate.label}</span>
            </div>
            <span className="text-muted-foreground text-xs shrink-0">
              {gate.value}
            </span>
          </div>
        ))}
      </div>
    </div>
  );
}

function buildCoreGates(workspace: Workspace, serviceStatus: ServiceStatus | null): GateRow[] {
  const gates: GateRow[] = [];

  // Cross-reference workspace flag with service status for honest reporting
  const liveConfigured = workspace.live_trading_enabled;
  const liveRunning = serviceStatus?.live_trading?.running ?? false;
  gates.push({
    label: 'Live Trading',
    status: liveConfigured && liveRunning
      ? 'green'
      : liveConfigured && !liveRunning
        ? 'red'
        : 'red',
    value: liveConfigured && liveRunning
      ? 'Running'
      : liveConfigured && !liveRunning
        ? serviceStatus?.live_trading?.reason || 'Offline'
        : 'Disabled',
  });

  // Wallet readiness: prefer the DB field, but fall back to the live_trading
  // service status which checks order_executor.is_live_ready() — this covers
  // wallets loaded from WALLET_PRIVATE_KEY/vault that never wrote back to DB.
  const walletFromDb = workspace.trading_wallet_address;
  const walletReady = walletFromDb || (serviceStatus?.live_trading?.running ?? false);
  gates.push({
    label: 'Wallet Loaded',
    status: walletReady ? 'green' : 'red',
    value: walletFromDb
      ? `${walletFromDb.slice(0, 6)}...`
      : walletReady
        ? 'Active'
        : 'None',
  });

  gates.push({
    label: 'Copy Trading',
    status: workspace.copy_trading_enabled ? 'green' : 'red',
    value: workspace.copy_trading_enabled ? 'Enabled' : 'Disabled',
  });

  // Cross-reference arb flag with service status for honest reporting
  const arbConfigured = workspace.arb_auto_execute;
  const arbRunning = serviceStatus?.arb_executor?.running ?? false;
  gates.push({
    label: 'Arb Auto-Execute',
    status: arbConfigured && arbRunning
      ? 'green'
      : arbConfigured && !arbRunning
        ? 'red'
        : 'amber',
    value: arbConfigured && arbRunning
      ? 'Running'
      : arbConfigured && !arbRunning
        ? serviceStatus?.arb_executor?.reason || 'Offline'
        : 'Manual',
  });

  // Exit handler gate
  const ehConfigured = workspace.exit_handler_enabled;
  const ehRunning = serviceStatus?.exit_handler?.running ?? false;
  gates.push({
    label: 'Exit Handler',
    status: ehConfigured && ehRunning
      ? 'green'
      : ehConfigured && !ehRunning
        ? 'red'
        : 'amber',
    value: ehConfigured && ehRunning
      ? 'Running'
      : ehConfigured && !ehRunning
        ? serviceStatus?.exit_handler?.reason || 'Offline'
        : 'Disabled',
  });

  const hasRpc = !!(workspace.polygon_rpc_url || workspace.alchemy_api_key);
  gates.push({
    label: 'RPC Configured',
    status: hasRpc ? 'green' : 'red',
    value: hasRpc ? 'Yes' : 'Missing',
  });

  return gates;
}

function buildCircuitBreakerGates(riskStatus: RiskStatus | null): GateRow[] {
  if (!riskStatus) return [{ label: 'Circuit Breaker', status: 'amber', value: 'Loading...' }];

  const cb = riskStatus.circuit_breaker;
  const cfg = cb.config;
  const gates: GateRow[] = [];

  gates.push({
    label: 'Circuit Breaker',
    status: !cfg.enabled ? 'amber' : cb.tripped ? 'red' : 'green',
    value: !cfg.enabled ? 'Disabled' : cb.tripped ? 'TRIPPED' : 'OK',
  });

  // Daily P&L vs limit
  const pnlPct = cfg.max_daily_loss > 0
    ? Math.abs(Math.min(cb.daily_pnl, 0)) / cfg.max_daily_loss
    : 0;
  gates.push({
    label: 'Daily P&L vs Limit',
    status: pnlPct >= 1 ? 'red' : pnlPct >= 0.7 ? 'amber' : 'green',
    value: `${formatCurrency(cb.daily_pnl)} / -${formatCurrency(cfg.max_daily_loss)}`,
  });

  // Consecutive losses
  const lossPct = cfg.max_consecutive_losses > 0
    ? cb.consecutive_losses / cfg.max_consecutive_losses
    : 0;
  gates.push({
    label: 'Consecutive Losses',
    status: lossPct >= 1 ? 'red' : lossPct >= 0.7 ? 'amber' : 'green',
    value: `${cb.consecutive_losses} / ${cfg.max_consecutive_losses}`,
  });

  // Drawdown proximity
  if (cb.peak_value > 0 && cfg.max_drawdown_pct > 0) {
    const currentDrawdown = (cb.peak_value - cb.current_value) / cb.peak_value;
    const drawdownPct = currentDrawdown / cfg.max_drawdown_pct;
    gates.push({
      label: 'Drawdown Proximity',
      status: drawdownPct >= 1 ? 'red' : drawdownPct >= 0.7 ? 'amber' : 'green',
      value: `${(currentDrawdown * 100).toFixed(1)}% / ${(cfg.max_drawdown_pct * 100).toFixed(0)}%`,
    });
  }

  return gates;
}

function buildCopyTradingGates(dynamicTunerStatus: DynamicTunerStatus | null): GateRow[] {
  if (!dynamicTunerStatus) return [];

  const gates: GateRow[] = [];
  const configs = dynamicTunerStatus.dynamic_config || [];

  for (const cfg of configs) {
    if (cfg.key === 'COPY_MIN_TRADE_VALUE') {
      gates.push({
        label: 'Min Trade Value',
        status: 'green',
        value: formatCurrency(cfg.current_value),
      });
    } else if (cfg.key === 'COPY_MAX_SLIPPAGE_PCT') {
      gates.push({
        label: 'Max Slippage',
        status: 'green',
        value: `${(cfg.current_value * 100).toFixed(2)}%`,
      });
    } else if (cfg.key === 'COPY_MAX_LATENCY_SECS') {
      const mins = Math.floor(cfg.current_value / 60);
      const secs = cfg.current_value % 60;
      gates.push({
        label: 'Max Trade Age',
        status: 'green',
        value: mins > 0 ? `${mins}m ${secs}s` : `${secs}s`,
      });
    }
  }

  return gates;
}

function buildServiceGates(serviceStatus: ServiceStatus | null): GateRow[] {
  if (!serviceStatus) return [{ label: 'Services', status: 'amber', value: 'Loading...' }];

  const gates: GateRow[] = [];
  const entries: [string, { running: boolean; reason?: string }][] = [
    ['Copy Trading Monitor', serviceStatus.copy_trading],
    ['Arb Executor', serviceStatus.arb_executor],
    ['Exit Handler', serviceStatus.exit_handler],
    ['Live Trading', serviceStatus.live_trading],
    ['Harvester', serviceStatus.harvester],
    ['Metrics Calculator', serviceStatus.metrics_calculator],
  ];

  for (const [label, svc] of entries) {
    gates.push({
      label,
      status: svc.running ? 'green' : 'red',
      value: svc.running ? 'Running' : svc.reason || 'Stopped',
    });
  }

  return gates;
}

export const TradingGatesPanel = memo(function TradingGatesPanel({
  workspace,
  serviceStatus,
  riskStatus,
  dynamicTunerStatus,
}: TradingGatesPanelProps) {
  const coreGates = buildCoreGates(workspace, serviceStatus);
  const cbGates = buildCircuitBreakerGates(riskStatus);
  const copyGates = buildCopyTradingGates(dynamicTunerStatus);
  const svcGates = buildServiceGates(serviceStatus);

  // Count issues
  const allGates = [...coreGates, ...cbGates, ...copyGates, ...svcGates];
  const redCount = allGates.filter((g) => g.status === 'red').length;
  const amberCount = allGates.filter((g) => g.status === 'amber').length;

  return (
    <Card>
      <CardHeader className="pb-3">
        <CardTitle className="flex items-center gap-2 text-base">
          <Activity className="h-5 w-5" />
          Trading Gates
          {redCount > 0 && (
            <span className="ml-auto inline-flex items-center rounded-full bg-red-500/10 border border-red-500/30 px-2 py-0.5 text-xs text-red-500">
              {redCount} blocked
            </span>
          )}
          {redCount === 0 && amberCount > 0 && (
            <span className="ml-auto inline-flex items-center rounded-full bg-yellow-500/10 border border-yellow-500/30 px-2 py-0.5 text-xs text-yellow-500">
              {amberCount} warning{amberCount !== 1 ? 's' : ''}
            </span>
          )}
          {redCount === 0 && amberCount === 0 && (
            <span className="ml-auto inline-flex items-center rounded-full bg-green-500/10 border border-green-500/30 px-2 py-0.5 text-xs text-green-500">
              All clear
            </span>
          )}
        </CardTitle>
        <CardDescription>
          At-a-glance view of every gate that can block or limit trading
        </CardDescription>
      </CardHeader>
      <CardContent>
        <div className="grid gap-5 sm:grid-cols-2">
          <GateSection title="Core Enablement" gates={coreGates} />
          <GateSection title="Circuit Breaker" gates={cbGates} />
          {copyGates.length > 0 && (
            <GateSection title="Copy Trading Thresholds" gates={copyGates} />
          )}
          <GateSection title="Services" gates={svcGates} />
        </div>
      </CardContent>
    </Card>
  );
});
