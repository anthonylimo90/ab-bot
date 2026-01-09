'use client';

import { useState, useCallback } from 'react';
import { useModeStore } from '@/stores/mode-store';
import { api } from '@/lib/api';
import type { BacktestParams, BacktestResult, EquityPoint } from '@/types/api';

interface UseBacktestReturn {
  results: BacktestResult | null;
  history: BacktestResult[];
  isRunning: boolean;
  error: string | null;
  runBacktest: (params: BacktestParams) => Promise<void>;
  loadHistory: () => Promise<void>;
  loadResult: (id: string) => Promise<void>;
}

// Generate mock equity curve
function generateMockEquityCurve(
  initialCapital: number,
  startDate: string,
  endDate: string,
  returnPct: number
): EquityPoint[] {
  const data: EquityPoint[] = [];
  const start = new Date(startDate);
  const end = new Date(endDate);
  const days = Math.ceil((end.getTime() - start.getTime()) / (24 * 60 * 60 * 1000));

  let value = initialCapital;
  const dailyReturn = Math.pow(1 + returnPct / 100, 1 / days) - 1;

  for (let i = 0; i <= days; i++) {
    const date = new Date(start.getTime() + i * 24 * 60 * 60 * 1000);
    // Add some volatility
    const noise = (Math.random() - 0.5) * 0.02;
    value = value * (1 + dailyReturn + noise);

    data.push({
      timestamp: date.toISOString().split('T')[0],
      value: Math.round(value * 100) / 100,
    });
  }

  return data;
}

// Generate mock backtest result
function generateMockResult(params: BacktestParams): BacktestResult {
  const totalReturnPct = 20 + Math.random() * 40; // 20-60%
  const finalValue = params.initial_capital * (1 + totalReturnPct / 100);
  const winRate = 55 + Math.random() * 20; // 55-75%
  const totalTrades = 50 + Math.floor(Math.random() * 100);
  const winningTrades = Math.floor(totalTrades * (winRate / 100));

  return {
    id: Math.random().toString(36).slice(2),
    strategy: params.strategy,
    start_date: params.start_date,
    end_date: params.end_date,
    initial_capital: params.initial_capital,
    final_value: Math.round(finalValue * 100) / 100,
    total_return: Math.round((finalValue - params.initial_capital) * 100) / 100,
    total_return_pct: Math.round(totalReturnPct * 100) / 100,
    annualized_return: Math.round(totalReturnPct * 1.2 * 100) / 100,
    sharpe_ratio: Math.round((1 + Math.random() * 2) * 100) / 100,
    sortino_ratio: Math.round((1.2 + Math.random() * 2) * 100) / 100,
    max_drawdown: Math.round((5 + Math.random() * 15) * 100) / 100,
    max_drawdown_pct: Math.round((5 + Math.random() * 15) * 100) / 100,
    total_trades: totalTrades,
    winning_trades: winningTrades,
    losing_trades: totalTrades - winningTrades,
    win_rate: Math.round(winRate * 100) / 100,
    avg_win: Math.round((50 + Math.random() * 100) * 100) / 100,
    avg_loss: Math.round((30 + Math.random() * 50) * 100) / 100,
    profit_factor: Math.round((1.5 + Math.random()) * 100) / 100,
    total_fees: Math.round(params.initial_capital * 0.01 * 100) / 100,
    created_at: new Date().toISOString(),
    status: 'completed',
    equity_curve: generateMockEquityCurve(
      params.initial_capital,
      params.start_date,
      params.end_date,
      totalReturnPct
    ),
  };
}

export function useBacktest(): UseBacktestReturn {
  const { mode } = useModeStore();
  const [results, setResults] = useState<BacktestResult | null>(null);
  const [history, setHistory] = useState<BacktestResult[]>([]);
  const [isRunning, setIsRunning] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const isLiveMode = mode === 'live';

  // Run a new backtest
  const runBacktest = useCallback(async (params: BacktestParams) => {
    setIsRunning(true);
    setError(null);

    if (!isLiveMode) {
      // Simulate delay
      await new Promise((resolve) => setTimeout(resolve, 2000));
      const mockResult = generateMockResult(params);
      setResults(mockResult);
      setHistory((prev) => [mockResult, ...prev]);
      setIsRunning(false);
      return;
    }

    try {
      const result = await api.runBacktest(params);

      // Poll for results if status is pending/running
      if (result.status === 'pending' || result.status === 'running') {
        const pollForResults = async () => {
          let attempts = 0;
          const maxAttempts = 60; // 5 minutes max

          while (attempts < maxAttempts) {
            await new Promise((resolve) => setTimeout(resolve, 5000));
            const updated = await api.getBacktestResult(result.id);

            if (updated.status === 'completed') {
              setResults(updated);
              setHistory((prev) => [updated, ...prev]);
              return;
            }

            if (updated.status === 'failed') {
              throw new Error(updated.error || 'Backtest failed');
            }

            attempts++;
          }

          throw new Error('Backtest timed out');
        };

        await pollForResults();
      } else {
        setResults(result);
        setHistory((prev) => [result, ...prev]);
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to run backtest');
      console.error('Failed to run backtest:', err);
    } finally {
      setIsRunning(false);
    }
  }, [isLiveMode]);

  // Load backtest history
  const loadHistory = useCallback(async () => {
    if (!isLiveMode) {
      // Generate some mock history
      const mockHistory: BacktestResult[] = Array.from({ length: 5 }, (_, i) => ({
        ...generateMockResult({
          strategy: { type: i % 2 === 0 ? 'CopyTrading' : 'Arbitrage' },
          start_date: new Date(Date.now() - (i + 1) * 30 * 24 * 60 * 60 * 1000).toISOString().split('T')[0],
          end_date: new Date(Date.now() - i * 30 * 24 * 60 * 60 * 1000).toISOString().split('T')[0],
          initial_capital: 1000,
        }),
        created_at: new Date(Date.now() - i * 24 * 60 * 60 * 1000).toISOString(),
      }));
      setHistory(mockHistory);
      return;
    }

    try {
      const data = await api.getBacktestResults({ limit: 20 });
      setHistory(data);
    } catch (err) {
      console.error('Failed to load backtest history:', err);
    }
  }, [isLiveMode]);

  // Load a specific result
  const loadResult = useCallback(async (id: string) => {
    if (!isLiveMode) {
      const found = history.find((r) => r.id === id);
      if (found) setResults(found);
      return;
    }

    try {
      const result = await api.getBacktestResult(id);
      setResults(result);
    } catch (err) {
      console.error('Failed to load backtest result:', err);
    }
  }, [isLiveMode, history]);

  return {
    results,
    history,
    isRunning,
    error,
    runBacktest,
    loadHistory,
    loadResult,
  };
}
