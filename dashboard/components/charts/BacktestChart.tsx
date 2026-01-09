'use client';

import { useEffect, useRef, useState } from 'react';
import { createChart, ColorType, IChartApi, ISeriesApi, LineData, Time } from 'lightweight-charts';
import { cn } from '@/lib/utils';

// Chart colors (hex values for TradingView compatibility)
const CHART_COLORS = {
  text: '#a1a1aa',        // muted-foreground
  border: '#27272a',      // border
  profit: '#22c55e',      // green-500
  loss: '#ef4444',        // red-500
  profitArea: 'rgba(34, 197, 94, 0.4)',
  profitAreaBottom: 'rgba(34, 197, 94, 0)',
  lossArea: 'rgba(239, 68, 68, 0.4)',
  lossAreaBottom: 'rgba(239, 68, 68, 0)',
};

interface BacktestChartProps {
  data: { time: string; value: number }[];
  height?: number;
  className?: string;
  baseline?: number;
}

export function BacktestChart({
  data,
  height = 300,
  className,
  baseline,
}: BacktestChartProps) {
  const chartContainerRef = useRef<HTMLDivElement>(null);
  const chartRef = useRef<IChartApi | null>(null);
  const [tooltipData, setTooltipData] = useState<{ time: string; value: number; pnl: number } | null>(null);

  const initialValue = baseline ?? (data.length > 0 ? data[0].value : 0);

  useEffect(() => {
    if (!chartContainerRef.current || data.length === 0) return;

    // Create chart
    const chart = createChart(chartContainerRef.current, {
      layout: {
        background: { type: ColorType.Solid, color: 'transparent' },
        textColor: CHART_COLORS.text,
      },
      grid: {
        vertLines: { color: CHART_COLORS.border },
        horzLines: { color: CHART_COLORS.border },
      },
      width: chartContainerRef.current.clientWidth,
      height,
      rightPriceScale: {
        borderColor: CHART_COLORS.border,
        scaleMargins: {
          top: 0.1,
          bottom: 0.1,
        },
      },
      timeScale: {
        borderColor: CHART_COLORS.border,
        timeVisible: true,
        secondsVisible: false,
      },
      crosshair: {
        mode: 1,
        vertLine: {
          width: 1,
          color: CHART_COLORS.text,
          style: 2,
        },
        horzLine: {
          width: 1,
          color: CHART_COLORS.text,
          style: 2,
        },
      },
    });

    chartRef.current = chart;

    // Add baseline series (initial capital line)
    const baselineSeries = chart.addLineSeries({
      color: CHART_COLORS.text,
      lineWidth: 1,
      lineStyle: 2, // Dashed
      priceLineVisible: false,
      lastValueVisible: false,
    });

    baselineSeries.setData([
      { time: data[0].time as Time, value: initialValue },
      { time: data[data.length - 1].time as Time, value: initialValue },
    ]);

    // Calculate if overall trend is positive
    const isPositive = data[data.length - 1].value >= initialValue;

    // Add main series
    const areaSeries = chart.addAreaSeries({
      lineColor: isPositive ? CHART_COLORS.profit : CHART_COLORS.loss,
      topColor: isPositive ? CHART_COLORS.profitArea : CHART_COLORS.lossArea,
      bottomColor: isPositive ? CHART_COLORS.profitAreaBottom : CHART_COLORS.lossAreaBottom,
      lineWidth: 2,
      priceFormat: {
        type: 'price',
        precision: 2,
        minMove: 0.01,
      },
    });

    // Format data for chart
    const chartData: LineData[] = data.map((d) => ({
      time: d.time as Time,
      value: d.value,
    }));

    areaSeries.setData(chartData);

    // Fit content
    chart.timeScale().fitContent();

    // Subscribe to crosshair move for tooltip
    chart.subscribeCrosshairMove((param) => {
      if (
        param.point === undefined ||
        !param.time ||
        param.point.x < 0 ||
        param.point.y < 0
      ) {
        setTooltipData(null);
        return;
      }

      const seriesData = param.seriesData.get(areaSeries);
      if (seriesData && 'value' in seriesData) {
        const value = seriesData.value as number;
        setTooltipData({
          time: param.time as string,
          value,
          pnl: value - initialValue,
        });
      }
    });

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
  }, [data, height, initialValue]);

  if (data.length === 0) {
    return (
      <div
        className={cn('flex items-center justify-center border-2 border-dashed rounded-lg text-muted-foreground', className)}
        style={{ height }}
      >
        No backtest data available
      </div>
    );
  }

  return (
    <div className={cn('relative', className)}>
      <div ref={chartContainerRef} />
      {tooltipData && (
        <div className="absolute top-2 left-2 bg-popover/90 backdrop-blur border rounded-lg px-3 py-2 text-sm shadow-lg">
          <div className="text-muted-foreground">{tooltipData.time}</div>
          <div className="font-bold tabular-nums">
            ${tooltipData.value.toLocaleString(undefined, { minimumFractionDigits: 2, maximumFractionDigits: 2 })}
          </div>
          <div className={cn('text-xs tabular-nums', tooltipData.pnl >= 0 ? 'text-profit' : 'text-loss')}>
            {tooltipData.pnl >= 0 ? '+' : ''}
            ${tooltipData.pnl.toLocaleString(undefined, { minimumFractionDigits: 2, maximumFractionDigits: 2 })}
            {' '}
            ({((tooltipData.pnl / initialValue) * 100).toFixed(1)}%)
          </div>
        </div>
      )}
    </div>
  );
}
