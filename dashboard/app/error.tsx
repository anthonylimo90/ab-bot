'use client';

import { useEffect } from 'react';
import { Card, CardContent } from '@/components/ui/card';
import { Button } from '@/components/ui/button';
import { AlertTriangle, RefreshCw, Home } from 'lucide-react';
import Link from 'next/link';

interface ErrorProps {
  error: Error & { digest?: string };
  reset: () => void;
}

export default function Error({ error, reset }: ErrorProps) {
  useEffect(() => {
    console.error('Page error:', error);
  }, [error]);

  return (
    <div className="flex items-center justify-center min-h-[60vh]">
      <Card className="max-w-md w-full border-loss/20 bg-loss/5">
        <CardContent className="p-8">
          <div className="flex flex-col items-center justify-center gap-6 text-center">
            <div className="rounded-full bg-loss/10 p-4">
              <AlertTriangle className="h-12 w-12 text-loss" />
            </div>
            <div className="space-y-2">
              <h2 className="font-semibold text-xl">Something went wrong</h2>
              <p className="text-sm text-muted-foreground">
                {error.message || 'An unexpected error occurred while loading this page.'}
              </p>
              {error.digest && (
                <p className="text-xs text-muted-foreground font-mono">
                  Error ID: {error.digest}
                </p>
              )}
            </div>
            <div className="flex gap-3">
              <Button onClick={reset} variant="default">
                <RefreshCw className="mr-2 h-4 w-4" />
                Try again
              </Button>
              <Button variant="outline" asChild>
                <Link href="/">
                  <Home className="mr-2 h-4 w-4" />
                  Go home
                </Link>
              </Button>
            </div>
          </div>
        </CardContent>
      </Card>
    </div>
  );
}
