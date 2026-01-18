'use client';

import { useEffect } from 'react';
import { useRouter } from 'next/navigation';

/**
 * This page has been merged with /portfolio.
 * Redirects to /portfolio for backwards compatibility.
 */
export default function PositionsPage() {
  const router = useRouter();

  useEffect(() => {
    router.replace('/portfolio');
  }, [router]);

  return (
    <div className="flex items-center justify-center min-h-[50vh]">
      <p className="text-muted-foreground">Redirecting to Portfolio...</p>
    </div>
  );
}
