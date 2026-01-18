'use client';

import { useState, useRef, useEffect } from 'react';
import Link from 'next/link';
import { usePathname, useRouter } from 'next/navigation';
import { Settings, User, LogOut, ChevronDown, ShieldAlert } from 'lucide-react';
import { ModeToggle } from './ModeToggle';
import { Button } from '@/components/ui/button';
import { cn } from '@/lib/utils';
import { useAuthStore } from '@/stores/auth-store';

const mobileNavItems = [
  { href: '/', label: 'Dashboard' },
  { href: '/portfolio', label: 'Portfolio' },
  { href: '/discover', label: 'Discover' },
  { href: '/allocate', label: 'Allocate' },
  { href: '/backtest', label: 'Backtest' },
];

export function Header() {
  const pathname = usePathname();
  const router = useRouter();
  const [isUserMenuOpen, setIsUserMenuOpen] = useState(false);
  const menuRef = useRef<HTMLDivElement>(null);
  const { user, logout } = useAuthStore();

  // Close menu when clicking outside
  useEffect(() => {
    function handleClickOutside(event: MouseEvent) {
      if (menuRef.current && !menuRef.current.contains(event.target as Node)) {
        setIsUserMenuOpen(false);
      }
    }
    document.addEventListener('mousedown', handleClickOutside);
    return () => document.removeEventListener('mousedown', handleClickOutside);
  }, []);

  const handleLogout = () => {
    logout();
    setIsUserMenuOpen(false);
    router.push('/login');
  };

  const userInitials = user?.name
    ? user.name.split(' ').map((n) => n[0]).join('').toUpperCase().slice(0, 2)
    : user?.email?.slice(0, 2).toUpperCase() || 'U';

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

          {/* User Menu */}
          <div className="relative" ref={menuRef}>
            <Button
              variant="ghost"
              size="sm"
              className="gap-1"
              onClick={() => setIsUserMenuOpen(!isUserMenuOpen)}
            >
              <div className="flex h-7 w-7 items-center justify-center rounded-full bg-primary text-primary-foreground text-xs font-medium">
                {userInitials}
              </div>
              <ChevronDown className={cn(
                'h-4 w-4 transition-transform',
                isUserMenuOpen && 'rotate-180'
              )} />
            </Button>

            {isUserMenuOpen && (
              <div className="absolute right-0 mt-2 w-56 rounded-md border bg-popover p-1 shadow-lg">
                <div className="px-3 py-2 border-b mb-1">
                  <p className="text-sm font-medium">{user?.name || 'User'}</p>
                  <p className="text-xs text-muted-foreground">{user?.email}</p>
                  <p className="text-xs text-muted-foreground capitalize mt-1">
                    Role: {user?.role}
                  </p>
                </div>
                {user?.role === 'Admin' && (
                  <Link
                    href="/admin"
                    className="flex items-center gap-2 px-3 py-2 text-sm rounded-sm hover:bg-accent cursor-pointer"
                    onClick={() => setIsUserMenuOpen(false)}
                  >
                    <ShieldAlert className="h-4 w-4 text-red-500" />
                    Admin Portal
                  </Link>
                )}
                <Link
                  href="/settings"
                  className="flex items-center gap-2 px-3 py-2 text-sm rounded-sm hover:bg-accent cursor-pointer"
                  onClick={() => setIsUserMenuOpen(false)}
                >
                  <Settings className="h-4 w-4" />
                  Settings
                </Link>
                <button
                  onClick={handleLogout}
                  className="flex w-full items-center gap-2 px-3 py-2 text-sm rounded-sm hover:bg-accent text-destructive"
                >
                  <LogOut className="h-4 w-4" />
                  Sign out
                </button>
              </div>
            )}
          </div>
        </div>
      </div>
    </header>
  );
}
