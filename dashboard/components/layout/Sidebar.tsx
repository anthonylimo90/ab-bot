'use client';

import Link from 'next/link';
import { usePathname } from 'next/navigation';
import {
  LayoutDashboard,
  Users,
  UserCheck,
  Briefcase,
  RefreshCw,
  Sliders,
  TestTube,
  Settings,
} from 'lucide-react';
import { cn } from '@/lib/utils';
import { useRosterStore } from '@/stores/roster-store';

const navItems = [
  { href: '/', label: 'Dashboard', icon: LayoutDashboard },
  { href: '/roster', label: 'Active 5', icon: Users, badge: 'roster' },
  { href: '/bench', label: 'Bench', icon: UserCheck, badge: 'bench' },
  { href: '/positions', label: 'Positions', icon: Briefcase },
  { href: '/rotation', label: 'Rotation', icon: RefreshCw },
  { href: '/allocate', label: 'Allocate', icon: Sliders },
  { href: '/backtest', label: 'Backtest', icon: TestTube },
  { href: '/settings', label: 'Settings', icon: Settings },
];

export function Sidebar() {
  const pathname = usePathname();
  const { activeWallets, benchWallets } = useRosterStore();

  const getBadgeCount = (badgeType?: string) => {
    if (badgeType === 'roster') return activeWallets.length;
    if (badgeType === 'bench') return benchWallets.length;
    return null;
  };

  return (
    <aside className="fixed left-0 top-16 z-30 hidden h-[calc(100vh-4rem)] w-64 border-r bg-background md:block">
      <nav className="flex flex-col gap-1 p-4">
        {navItems.map((item) => {
          const isActive = pathname === item.href ||
            (item.href !== '/' && pathname.startsWith(item.href));
          const Icon = item.icon;
          const badgeCount = getBadgeCount(item.badge);

          return (
            <Link
              key={item.href}
              href={item.href}
              className={cn(
                'flex items-center justify-between rounded-lg px-3 py-2 text-sm font-medium transition-colors',
                isActive
                  ? 'bg-primary text-primary-foreground'
                  : 'text-muted-foreground hover:bg-accent hover:text-accent-foreground'
              )}
            >
              <div className="flex items-center gap-3">
                <Icon className="h-4 w-4" />
                {item.label}
              </div>
              {badgeCount !== null && badgeCount > 0 && (
                <span
                  className={cn(
                    'flex h-5 min-w-5 items-center justify-center rounded-full px-1.5 text-xs font-medium',
                    isActive
                      ? 'bg-primary-foreground/20 text-primary-foreground'
                      : 'bg-muted text-muted-foreground'
                  )}
                >
                  {badgeCount}
                </span>
              )}
            </Link>
          );
        })}
      </nav>

      {/* Roster Summary */}
      <div className="absolute bottom-4 left-4 right-4">
        <div className="rounded-lg border bg-muted/30 p-3 space-y-2">
          <div className="flex items-center justify-between text-xs">
            <span className="text-muted-foreground">Active Roster</span>
            <span className="font-medium">{activeWallets.length}/5</span>
          </div>
          <div className="w-full bg-muted rounded-full h-1.5">
            <div
              className="bg-primary h-1.5 rounded-full transition-all"
              style={{ width: `${(activeWallets.length / 5) * 100}%` }}
            />
          </div>
          {activeWallets.length < 5 && (
            <p className="text-xs text-muted-foreground">
              {5 - activeWallets.length} slot{5 - activeWallets.length !== 1 ? 's' : ''} available
            </p>
          )}
        </div>
      </div>
    </aside>
  );
}
