"use client";

import Link from "next/link";
import * as DialogPrimitive from "@radix-ui/react-dialog";
import {
  LogOut,
  Settings2,
  Target,
  Pause,
  Play,
  Wallet,
  Plus,
  X,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { cn } from "@/lib/utils";
import { primaryNavSections } from "@/components/layout/navigation";

interface MobileNavDrawerProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  pathname: string;
  userName?: string | null;
  userEmail?: string | null;
  userRole?: string | null;
  hasWallet: boolean;
  walletLabel?: string | null;
  walletSummary: string;
  showWorkspaceDetails: boolean;
  modeLabel: string;
  isTradingActive: boolean;
  isTradingPending: boolean;
  onToggleTrading: () => void;
  onConnectWallet: () => void;
  onLogout: () => void;
}

export function MobileNavDrawer({
  open,
  onOpenChange,
  pathname,
  userName,
  userEmail,
  userRole,
  hasWallet,
  walletLabel,
  walletSummary,
  showWorkspaceDetails,
  modeLabel,
  isTradingActive,
  isTradingPending,
  onToggleTrading,
  onConnectWallet,
  onLogout,
}: MobileNavDrawerProps) {
  const ModeIcon = modeLabel === "Guided" ? Target : Settings2;

  return (
    <DialogPrimitive.Root open={open} onOpenChange={onOpenChange}>
      <DialogPrimitive.Portal>
        <DialogPrimitive.Overlay className="fixed inset-0 z-50 bg-black/60 backdrop-blur-sm" />
        <DialogPrimitive.Content
          className={cn(
            "fixed inset-y-0 left-0 z-50 flex h-dvh w-[min(24rem,calc(100vw-1rem))] flex-col border-r bg-background shadow-xl outline-none",
            "data-[state=open]:animate-in data-[state=closed]:animate-out",
          )}
        >
          <div className="flex items-center justify-between border-b px-4 py-4">
            <div className="space-y-0.5">
              <DialogPrimitive.Title className="text-base font-semibold">
                Navigation
              </DialogPrimitive.Title>
              <DialogPrimitive.Description className="text-xs text-muted-foreground">
                Browse dashboard pages and account controls.
              </DialogPrimitive.Description>
            </div>
            <DialogPrimitive.Close asChild>
              <Button variant="ghost" size="icon" aria-label="Close navigation menu">
                <X className="h-5 w-5" />
              </Button>
            </DialogPrimitive.Close>
          </div>

          <div className="flex-1 overflow-y-auto px-3 py-4">
            <nav className="space-y-6">
              {primaryNavSections.map((section) => (
                <div key={section.title} className="space-y-2">
                  <h3 className="px-3 text-xs font-semibold uppercase tracking-wider text-muted-foreground">
                    {section.title}
                  </h3>
                  <div className="space-y-1">
                    {section.items.map((item) => {
                      const isActive =
                        pathname === item.href ||
                        (item.href !== "/" && pathname.startsWith(item.href));
                      const Icon = item.icon;

                      return (
                        <Link
                          key={item.href}
                          href={item.href}
                          onClick={() => onOpenChange(false)}
                          className={cn(
                            "flex items-center gap-3 rounded-lg px-3 py-2.5 text-sm font-medium transition-colors",
                            isActive
                              ? "bg-primary text-primary-foreground"
                              : "text-muted-foreground hover:bg-accent hover:text-accent-foreground",
                          )}
                        >
                          <Icon className="h-4 w-4" />
                          {item.label}
                        </Link>
                      );
                    })}
                  </div>
                </div>
              ))}
            </nav>

            <div className="mt-6 space-y-4 border-t pt-4">
              {showWorkspaceDetails && (
                <>
                  <div className="rounded-xl border bg-muted/30 p-3">
                    <div className="flex flex-wrap items-center gap-2">
                      <Badge variant="secondary" className="gap-1">
                        <ModeIcon className="h-3 w-3" />
                        {modeLabel}
                      </Badge>
                      <Badge
                        variant="outline"
                        className={cn(
                          isTradingActive
                            ? "border-profit/30 bg-profit/10 text-profit"
                            : "border-amber-500/30 bg-amber-500/10 text-amber-700 dark:text-amber-300",
                        )}
                      >
                        {isTradingActive ? "Trading active" : "Trading paused"}
                      </Badge>
                    </div>

                    <Button
                      variant="outline"
                      className={cn(
                        "mt-3 w-full justify-start gap-2",
                        isTradingActive
                          ? "text-emerald-600 hover:text-amber-600 dark:text-emerald-400 dark:hover:text-amber-400"
                          : "text-amber-600 hover:text-emerald-600 dark:text-amber-400 dark:hover:text-emerald-400",
                      )}
                      disabled={isTradingPending}
                      onClick={onToggleTrading}
                    >
                      {isTradingActive ? (
                        <Pause className="h-4 w-4" />
                      ) : (
                        <Play className="h-4 w-4" />
                      )}
                      {isTradingPending
                        ? "Updating trading state..."
                        : isTradingActive
                          ? "Pause trading"
                          : "Resume trading"}
                    </Button>
                  </div>
                </>
              )}

              <div className="rounded-xl border bg-card p-3">
                <p className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
                  Account
                </p>
                <div className="mt-2 space-y-1.5 text-sm">
                  <p className="font-medium">{userName || "User"}</p>
                  {userEmail && (
                    <p className="break-all text-muted-foreground">{userEmail}</p>
                  )}
                  <p className="text-muted-foreground capitalize">
                    Role: {userRole || "member"}
                  </p>
                </div>
              </div>

              <div className="rounded-xl border bg-card p-3">
                <div className="flex items-center justify-between gap-3">
                  <div className="space-y-1">
                    <p className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
                      Wallet
                    </p>
                    <p className="text-sm font-medium">{walletSummary}</p>
                    {hasWallet && walletLabel && (
                      <p className="break-all text-xs text-muted-foreground">
                        {walletLabel}
                      </p>
                    )}
                  </div>
                  <div className="flex h-9 w-9 shrink-0 items-center justify-center rounded-full bg-muted">
                    <Wallet className="h-4 w-4 text-muted-foreground" />
                  </div>
                </div>
                {!hasWallet && (
                  <Button
                    variant="outline"
                    className="mt-3 w-full justify-start gap-2"
                    onClick={() => {
                      onOpenChange(false);
                      onConnectWallet();
                    }}
                  >
                    <Plus className="h-4 w-4" />
                    Connect wallet
                  </Button>
                )}
              </div>
            </div>
          </div>

          <div className="border-t px-3 py-3">
            <Button
              variant="ghost"
              className="w-full justify-start gap-2 text-destructive hover:bg-accent hover:text-destructive"
              onClick={onLogout}
            >
              <LogOut className="h-4 w-4" />
              Sign out
            </Button>
          </div>
        </DialogPrimitive.Content>
      </DialogPrimitive.Portal>
    </DialogPrimitive.Root>
  );
}
