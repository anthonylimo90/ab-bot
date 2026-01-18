'use client';

import Link from 'next/link';
import { usePathname } from 'next/navigation';
import { Building2, Users } from 'lucide-react';
import { cn } from '@/lib/utils';

const navItems = [
  { href: '/admin/workspaces', label: 'Workspaces', icon: Building2 },
  { href: '/admin/users', label: 'Users', icon: Users },
];

export function AdminSidebar() {
  const pathname = usePathname();

  return (
    <aside className="fixed left-0 top-16 z-30 hidden h-[calc(100vh-4rem)] w-64 border-r bg-background md:block">
      <div className="flex flex-col h-full">
        {/* Admin Portal Header */}
        <div className="p-4 border-b">
          <h2 className="text-lg font-semibold">Admin Portal</h2>
          <p className="text-xs text-muted-foreground">Platform management</p>
        </div>

        {/* Navigation */}
        <nav className="flex flex-col gap-1 p-4">
          {navItems.map((item) => {
            const isActive = pathname.startsWith(item.href);
            const Icon = item.icon;

            return (
              <Link
                key={item.href}
                href={item.href}
                className={cn(
                  'flex items-center gap-3 rounded-lg px-3 py-2 text-sm font-medium transition-colors',
                  isActive
                    ? 'bg-primary text-primary-foreground'
                    : 'text-muted-foreground hover:bg-accent hover:text-accent-foreground'
                )}
              >
                <Icon className="h-4 w-4" />
                {item.label}
              </Link>
            );
          })}
        </nav>
      </div>
    </aside>
  );
}
