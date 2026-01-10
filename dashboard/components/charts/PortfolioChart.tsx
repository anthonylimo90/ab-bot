'use client';

import { useEffect, useRef, useState, memo } from 'react';
import {
  createChart,
  ColorType,
  IChartApi,
  ISeriesApi,
  LineData,
  Time,
} from 'lightweight-charts';
import { cn } from '@/lib/utils';

// Chart colors (hex values for TradingView compatibility)
const CHART_COLORS = {
  text: '#a1a1aa', // muted-foreground
  border: '#27272a', // border
  profit: '#22c55e', // green-500
  loss: '#ef4444', // red-500
  profitArea: 'rgba(34, 197, 94, 0.4)',
  profitAreaBottom: 'rgba(34, 197, 94, 0)',
  lossArea: 'rgba(239, 68, 68, 0.4)',
  lossAreaBottom: 'rgba(239, 68, 68, 0)',
} as const;

interface PortfolioChartProps {
  data: { time: string; value: number }[];
  height?: number;
  className?: string;
  showTooltip?: boolean;
  period?: '1D' | '7D' | '30D' | 'ALL';
}

export const PortfolioChart = memo(function PortfolioChart({
  data,
  height = 300,
  className,
  showTooltip = true,
}: PortfolioChartProps) {
  const chartContainerRef = useRef<HTMLDivElement>(null);
  const chartRef = useRef<IChartApi | null>(null);
  const seriesRef = useRef<ISeriesApi<'Area'> | null>(null);
  const [tooltipData, setTooltipData] = useState<{
    time: string;
    value: number;
  } | null>(null);

  // Initialize chart only once
  useEffect(() => {
    if (!chartContainerRef.current) return;

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
      },
      timeScale: {
        borderColor: CHART_COLORS.border,
        timeVisible: true,
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

    // Add area series
    const areaSeries = chart.addAreaSeries({
      lineWidth: 2,
      priceFormat: {
        type: 'price',
        precision: 2,
        minMove: 0.01,
      },
    });

    seriesRef.current = areaSeries;

    // Subscribe to crosshair move for tooltip
    if (showTooltip) {
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
          setTooltipData({
            time: param.time as string,
            value: seriesData.value as number,
          });
        }
      });
    }

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
  }, [height, showTooltip]); // Only recreate on height/showTooltip change

  // Update data without recreating chart
  useEffect(() => {
    if (!seriesRef.current || !chartRef.current) return;

    // Calculate if overall trend is positive
    const isPositive =
      data.length >= 2 && data[data.length - 1].value >= data[0].value;

    // Update series colors based on trend
    seriesRef.current.applyOptions({
      lineColor: isPositive ? CHART_COLORS.profit : CHART_COLORS.loss,
      topColor: isPositive ? CHART_COLORS.profitArea : CHART_COLORS.lossArea,
      bottomColor: isPositive
        ? CHART_COLORS.profitAreaBottom
        : CHART_COLORS.lossAreaBottom,
    });

    // Format data for chart
    const chartData: LineData[] = data.map((d) => ({
      time: d.time as Time,
      value: d.value,
    }));

    seriesRef.current.setData(chartData);

    // Fit content
    chartRef.current.timeScale().fitContent();
  }, [data]);

  return (
    <div className={cn('relative', className)}>
      <div ref={chartContainerRef} />
      {showTooltip && tooltipData && (
        <div className="absolute top-2 left-2 bg-popover/90 backdrop-blur border rounded-lg px-3 py-2 text-sm shadow-lg">
          <div className="text-muted-foreground">{tooltipData.time}</div>
          <div className="font-bold tabular-nums">
            $
            {tooltipData.value.toLocaleString(undefined, {
              minimumFractionDigits: 2,
              maximumFractionDigits: 2,
            })}
          </div>
        </div>
      )}
    </div>
  );
});
