'use client';

import { useEffect, useRef } from 'react';
import { createChart, ColorType, IChartApi } from 'lightweight-charts';
import { cn } from '@/lib/utils';

// Chart colors (hex values for TradingView compatibility)
const CHART_COLORS = {
  profit: '#22c55e',      // green-500
  loss: '#ef4444',        // red-500
};

interface EquityCurveProps {
  data: { time: string; value: number }[];
  height?: number;
  className?: string;
  showAxes?: boolean;
}

export function EquityCurve({
  data,
  height = 60,
  className,
  showAxes = false,
}: EquityCurveProps) {
  const chartContainerRef = useRef<HTMLDivElement>(null);
  const chartRef = useRef<IChartApi | null>(null);

  useEffect(() => {
    if (!chartContainerRef.current || data.length === 0) return;

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

    // Calculate if overall trend is positive
    const isPositive = data[data.length - 1].value >= data[0].value;

    // Add line series (simpler than area for small charts)
    const lineSeries = chart.addLineSeries({
      color: isPositive ? CHART_COLORS.profit : CHART_COLORS.loss,
      lineWidth: 2,
      priceLineVisible: false,
      lastValueVisible: false,
    });

    // Format data
    const chartData = data.map((d) => ({
      time: d.time,
      value: d.value,
    }));

    lineSeries.setData(chartData as any);

    // Fit content
    chart.timeScale().fitContent();

    // Handle resize
    const handleResize = () => {
      if (chartContainerRef.current) {
        chart.applyOptions({ width: chartContainerRef.current.clientWidth });
      }
    };

    window.addEventListener('resize', handleResize);

    return () => {
      window.removeEventListener('resize', handleResize);
      chart.remove();
    };
  }, [data, height, showAxes]);

  if (data.length === 0) {
    return (
      <div
        className={cn('flex items-center justify-center text-muted-foreground text-xs', className)}
        style={{ height }}
      >
        No data
      </div>
    );
  }

  return <div ref={chartContainerRef} className={cn('', className)} />;
}
