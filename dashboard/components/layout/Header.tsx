'use client';

import Link from 'next/link';
import { usePathname } from 'next/navigation';
import { Menu, Settings, User } from 'lucide-react';
import { ModeToggle } from './ModeToggle';
import { Button } from '@/components/ui/button';
import { cn } from '@/lib/utils';

const mobileNavItems = [
  { href: '/', label: 'Dashboard' },
  { href: '/portfolio', label: 'Portfolio' },
  { href: '/discover', label: 'Discover' },
  { href: '/allocate', label: 'Allocate' },
  { href: '/backtest', label: 'Backtest' },
];

export function Header() {
  const pathname = usePathname();

  return (
    <header className="sticky top-0 z-40 w-full border-b bg-background/95 backdrop-blur supports-[backdrop-filter]:bg-background/60">
      <div className="flex h-16 items-center justify-between px-4 md:px-6">
        {/* Logo & Brand */}
        <div className="flex items-center gap-4">
          <Link href="/" className="flex items-center gap-2">
            <div className="flex h-8 w-8 items-center justify-center rounded-lg bg-primary text-primary-foreground font-bold">
              AB
            </div>
            <span className="hidden font-semibold sm:inline-block">
              AB-Bot
            </span>
          </Link>
        </div>

        {/* Mobile Nav */}
        <nav className="flex items-center gap-1 overflow-x-auto md:hidden">
          {mobileNavItems.map((item) => (
            <Link
              key={item.href}
              href={item.href}
              className={cn(
                'rounded-md px-3 py-1.5 text-sm font-medium whitespace-nowrap transition-colors',
                pathname === item.href
                  ? 'bg-primary text-primary-foreground'
                  : 'text-muted-foreground hover:bg-accent'
              )}
            >
              {item.label}
            </Link>
          ))}
        </nav>

        {/* Mode Toggle & Actions */}
        <div className="flex items-center gap-2">
          <ModeToggle />

          <Link href="/settings">
            <Button variant="ghost" size="icon" className="hidden sm:flex">
              <Settings className="h-4 w-4" />
            </Button>
          </Link>

          <Button variant="ghost" size="icon">
            <User className="h-4 w-4" />
          </Button>
        </div>
      </div>
    </header>
  );
}
