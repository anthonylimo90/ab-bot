'use client';

import { PieChart, Pie, Cell, ResponsiveContainer, Tooltip, Legend } from 'recharts';
import { cn } from '@/lib/utils';

interface AllocationData {
  name: string;
  value: number;
  color: string;
}

interface AllocationPieProps {
  data: AllocationData[];
  totalBalance?: number;
  className?: string;
  showLegend?: boolean;
  innerRadius?: number;
  outerRadius?: number;
}

const CustomTooltip = ({ active, payload, totalBalance = 10000 }: any) => {
  if (active && payload && payload.length) {
    const data = payload[0].payload;
    return (
      <div className="bg-popover/90 backdrop-blur border rounded-lg px-3 py-2 text-sm shadow-lg">
        <div className="font-medium">{data.name}</div>
        <div className="text-muted-foreground">
          {data.value.toFixed(1)}% (${((data.value / 100) * totalBalance).toLocaleString()})
        </div>
      </div>
    );
  }
  return null;
};

export function AllocationPie({
  data,
  totalBalance = 10000,
  className,
  showLegend = true,
  innerRadius = 60,
  outerRadius = 80,
}: AllocationPieProps) {
  const total = data.reduce((sum, d) => sum + d.value, 0);

  return (
    <div className={cn('w-full', className)}>
      <ResponsiveContainer width="100%" height={200}>
        <PieChart>
          <Pie
            data={data}
            cx="50%"
            cy="50%"
            innerRadius={innerRadius}
            outerRadius={outerRadius}
            paddingAngle={2}
            dataKey="value"
            animationBegin={0}
            animationDuration={500}
          >
            {data.map((entry, index) => (
              <Cell key={`cell-${index}`} fill={entry.color} strokeWidth={0} />
            ))}
          </Pie>
          <Tooltip content={<CustomTooltip totalBalance={totalBalance} />} />
        </PieChart>
      </ResponsiveContainer>

      {showLegend && (
        <div className="mt-4 space-y-2">
          {data.map((entry, index) => (
            <div key={index} className="flex items-center justify-between text-sm">
              <div className="flex items-center gap-2">
                <div
                  className="h-3 w-3 rounded-full"
                  style={{ backgroundColor: entry.color }}
                />
                <span>{entry.name}</span>
              </div>
              <span className="font-medium tabular-nums">{entry.value.toFixed(0)}%</span>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
