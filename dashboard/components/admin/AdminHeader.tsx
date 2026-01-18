'use client';

import { useState, useRef, useEffect } from 'react';
import Link from 'next/link';
import { useRouter } from 'next/navigation';
import { LogOut, ChevronDown, ShieldAlert } from 'lucide-react';
import { ModeToggle } from '@/components/layout/ModeToggle';
import { Button } from '@/components/ui/button';
import { cn } from '@/lib/utils';
import { useAuthStore } from '@/stores/auth-store';

export function AdminHeader() {
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
    router.push('/admin/login');
  };

  const userInitials = user?.name
    ? user.name.split(' ').map((n) => n[0]).join('').toUpperCase().slice(0, 2)
    : user?.email?.slice(0, 2).toUpperCase() || 'A';

  return (
    <header className="sticky top-0 z-40 w-full border-b bg-background/95 backdrop-blur supports-[backdrop-filter]:bg-background/60">
      <div className="flex h-16 items-center justify-between px-4 md:px-6">
        {/* Logo & Brand */}
        <div className="flex items-center gap-4">
          <Link href="/admin/workspaces" className="flex items-center gap-2">
            <div className="flex h-8 w-8 items-center justify-center rounded-lg bg-red-600 text-white font-bold">
              <ShieldAlert className="h-5 w-5" />
            </div>
            <span className="hidden font-semibold sm:inline-block">
              Admin Portal
            </span>
          </Link>
        </div>

        {/* Mode Toggle & Actions */}
        <div className="flex items-center gap-2">
          <ModeToggle />

          {/* User Menu */}
          <div className="relative" ref={menuRef}>
            <Button
              variant="ghost"
              size="sm"
              className="gap-1"
              onClick={() => setIsUserMenuOpen(!isUserMenuOpen)}
            >
              <div className="flex h-7 w-7 items-center justify-center rounded-full bg-red-600 text-white text-xs font-medium">
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
                  <p className="text-sm font-medium">{user?.name || 'Admin'}</p>
                  <p className="text-xs text-muted-foreground">{user?.email}</p>
                  <p className="text-xs text-red-500 font-medium mt-1">
                    Platform Administrator
                  </p>
                </div>
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
