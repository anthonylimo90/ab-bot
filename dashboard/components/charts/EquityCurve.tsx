'use client';

import { useEffect, useRef, memo, useCallback } from 'react';
import { createChart, ColorType, IChartApi, ISeriesApi } from 'lightweight-charts';
import { cn } from '@/lib/utils';

// Chart colors (hex values for TradingView compatibility)
const CHART_COLORS = {
  profit: '#22c55e', // green-500
  loss: '#ef4444', // red-500
} as const;

interface EquityCurveProps {
  data: { time: string; value: number }[];
  height?: number;
  className?: string;
  showAxes?: boolean;
}

export const EquityCurve = memo(function EquityCurve({
  data,
  height = 60,
  className,
  showAxes = false,
}: EquityCurveProps) {
  const chartContainerRef = useRef<HTMLDivElement>(null);
  const chartRef = useRef<IChartApi | null>(null);
  const seriesRef = useRef<ISeriesApi<'Line'> | null>(null);

  // Initialize chart only once
  useEffect(() => {
    if (!chartContainerRef.current) return;

    // Create minimal chart for sparkline effect
    const chart = createChart(chartContainerRef.current, {
      layout: {
        background: { type: ColorType.Solid, color: 'transparent' },
        textColor: 'transparent',
      },
      grid: {
        vertLines: { visible: false },
        horzLines: { visible: false },
      },
      width: chartContainerRef.current.clientWidth,
      height,
      rightPriceScale: {
        visible: showAxes,
        borderVisible: false,
      },
      leftPriceScale: {
        visible: false,
      },
      timeScale: {
        visible: showAxes,
        borderVisible: false,
      },
      crosshair: {
        mode: 0, // Disabled
      },
      handleScale: false,
      handleScroll: false,
    });

    chartRef.current = chart;

    // Create line series
    const lineSeries = chart.addLineSeries({
      lineWidth: 2,
      priceLineVisible: false,
      lastValueVisible: false,
    });

    seriesRef.current = lineSeries;

    // Handle resize
    const handleResize = () => {
      if (chartContainerRef.current && chartRef.current) {
        chartRef.current.applyOptions({
          width: chartContainerRef.current.clientWidth,
        });
      }
    };

    window.addEventListener('resize', handleResize);

    return () => {
      window.removeEventListener('resize', handleResize);
      chart.remove();
      chartRef.current = null;
      seriesRef.current = null;
    };
  }, [height, showAxes]); // Only recreate on height/showAxes change

  // Update data without recreating chart
  useEffect(() => {
    if (!seriesRef.current || !chartRef.current || data.length === 0) return;

    // Calculate if overall trend is positive
    const isPositive = data[data.length - 1].value >= data[0].value;

    // Update series color based on trend
    seriesRef.current.applyOptions({
      color: isPositive ? CHART_COLORS.profit : CHART_COLORS.loss,
    });

    // Update data
    const chartData = data.map((d) => ({
      time: d.time,
      value: d.value,
    }));

    seriesRef.current.setData(chartData as any);

    // Fit content
    chartRef.current.timeScale().fitContent();
  }, [data]);

  if (data.length === 0) {
    return (
      <div
        className={cn(
          'flex items-center justify-center text-muted-foreground text-xs',
          className
        )}
        style={{ height }}
      >
        No data
      </div>
    );
  }

  return <div ref={chartContainerRef} className={cn('', className)} />;
});
