'use client';

import type { ReactNode } from 'react';
import { Card, CardContent } from '@/components/ui/card';
import { InfoTooltip } from '@/components/shared/InfoTooltip';

interface PageIntroProps {
  title: string;
  description: string;
  bullets?: ReactNode[];
}

export function PageIntro({ title, description, bullets = [] }: PageIntroProps) {
  return (
    <Card className="border-dashed bg-muted/20">
      <CardContent className="space-y-3 p-4">
        <div className="flex items-center gap-2">
          <p className="text-sm font-semibold">{title}</p>
          <InfoTooltip content="This panel explains the page in plain language so you can understand what the system is doing before you take action." />
        </div>
        <p className="text-sm text-muted-foreground">{description}</p>
        {bullets.length > 0 && (
          <ul className="space-y-2 text-sm text-muted-foreground">
            {bullets.map((bullet, index) => (
              <li key={index} className="flex gap-2">
                <span className="mt-1 h-1.5 w-1.5 rounded-full bg-primary/70" />
                <span>{bullet}</span>
              </li>
            ))}
          </ul>
        )}
      </CardContent>
    </Card>
  );
}
