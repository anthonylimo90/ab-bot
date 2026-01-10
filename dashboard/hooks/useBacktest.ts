'use client';

import { useState, useCallback } from 'react';
import { api } from '@/lib/api';
import type { BacktestParams, BacktestResult } from '@/types/api';

interface UseBacktestReturn {
  results: BacktestResult | null;
  history: BacktestResult[];
  isRunning: boolean;
  error: string | null;
  runBacktest: (params: BacktestParams) => Promise<void>;
  loadHistory: () => Promise<void>;
  loadResult: (id: string) => Promise<void>;
}

export function useBacktest(): UseBacktestReturn {
  const [results, setResults] = useState<BacktestResult | null>(null);
  const [history, setHistory] = useState<BacktestResult[]>([]);
  const [isRunning, setIsRunning] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Run a new backtest
  const runBacktest = useCallback(async (params: BacktestParams) => {
    setIsRunning(true);
    setError(null);

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
  }, []);

  // Load backtest history
  const loadHistory = useCallback(async () => {
    try {
      const data = await api.getBacktestResults({ limit: 20 });
      setHistory(data);
    } catch (err) {
      console.error('Failed to load backtest history:', err);
    }
  }, []);

  // Load a specific result
  const loadResult = useCallback(async (id: string) => {
    try {
      const result = await api.getBacktestResult(id);
      setResults(result);
    } catch (err) {
      console.error('Failed to load backtest result:', err);
    }
  }, []);

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
