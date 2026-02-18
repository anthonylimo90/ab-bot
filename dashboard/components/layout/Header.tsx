"use client";

import { useState, useRef, useEffect } from "react";
import Link from "next/link";
import { usePathname, useRouter } from "next/navigation";
import {
  Settings,
  LogOut,
  ChevronDown,
  Menu,
  X,
  Target,
  Settings2,
  LayoutDashboard,
  Search,
  Eye,
  Star,
  PieChart,
  TrendingUp,
  Wallet,
  Plus,
  Pause,
  Play,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { cn } from "@/lib/utils";
import { useAuthStore } from "@/stores/auth-store";
import { useWorkspaceStore } from "@/stores/workspace-store";
import {
  useWalletStore,
  selectHasConnectedWallet,
  selectPrimaryWallet,
} from "@/stores/wallet-store";
import { useMutation } from "@tanstack/react-query";
import { useActivity } from "@/hooks/useActivity";
import { useWalletBalanceQuery } from "@/hooks/queries/useWalletsQuery";
import { ConnectionStatus } from "@/components/shared/ConnectionStatus";
import { ConnectWalletModal } from "@/components/wallet/ConnectWalletModal";
import { useToastStore } from "@/stores/toast-store";
import api from "@/lib/api";

const mobileNavSections = [
  {
    title: "Overview",
    items: [{ href: "/", label: "Dashboard", icon: LayoutDashboard }],
  },
  {
    title: "Copy Trading",
    items: [
      { href: "/discover", label: "Discover", icon: Search },
      { href: "/bench", label: "Watching", icon: Eye },
      { href: "/roster", label: "Active", icon: Star },
    ],
  },
  {
    title: "Portfolio",
    items: [
      { href: "/portfolio", label: "Positions", icon: PieChart },
      { href: "/backtest", label: "Backtest", icon: TrendingUp },
    ],
  },
  {
    title: "Settings",
    items: [{ href: "/settings", label: "Settings", icon: Settings }],
  },
];

export function Header() {
  const pathname = usePathname();
  const router = useRouter();
  const [isUserMenuOpen, setIsUserMenuOpen] = useState(false);
  const [isMobileMenuOpen, setIsMobileMenuOpen] = useState(false);
  const [showConnectModal, setShowConnectModal] = useState(false);
  const menuRef = useRef<HTMLDivElement>(null);
  const mobileMenuRef = useRef<HTMLDivElement>(null);
  const { user, logout } = useAuthStore();
  const { currentWorkspace, setCurrentWorkspace } = useWorkspaceStore();
  const toast = useToastStore();
  const { status: wsStatus } = useActivity();

  const isTradingActive =
    currentWorkspace?.copy_trading_enabled ||
    currentWorkspace?.live_trading_enabled ||
    false;

  const toggleTradingMutation = useMutation({
    mutationFn: async () => {
      if (!currentWorkspace) throw new Error("No workspace");
      const pausing = isTradingActive;
      return api.updateWorkspace(currentWorkspace.id, {
        copy_trading_enabled: !pausing,
        live_trading_enabled: !pausing,
      });
    },
    onSuccess: (updatedWorkspace) => {
      setCurrentWorkspace(updatedWorkspace);
      const paused =
        !updatedWorkspace.copy_trading_enabled &&
        !updatedWorkspace.live_trading_enabled;
      if (paused) {
        toast.warning(
          "Trading paused",
          "All automated trading has been paused",
        );
      } else {
        toast.success("Trading resumed", "Automated trading is now active");
      }
    },
    onError: () => {
      toast.error("Failed to update trading state");
    },
  });
  const hasWallet = useWalletStore(selectHasConnectedWallet);
  const primaryWallet = useWalletStore(selectPrimaryWallet);
  const { data: walletBalance } = useWalletBalanceQuery(
    primaryWallet?.address ?? null,
  );

  const modeLabel =
    currentWorkspace?.setup_mode === "automatic" ? "Guided" : "Custom";
  const ModeIcon =
    currentWorkspace?.setup_mode === "automatic" ? Target : Settings2;

  useEffect(() => {
    function handleClickOutside(event: MouseEvent) {
      if (menuRef.current && !menuRef.current.contains(event.target as Node)) {
        setIsUserMenuOpen(false);
      }
      if (
        mobileMenuRef.current &&
        !mobileMenuRef.current.contains(event.target as Node)
      ) {
        setIsMobileMenuOpen(false);
      }
    }
    document.addEventListener("mousedown", handleClickOutside);
    return () => document.removeEventListener("mousedown", handleClickOutside);
  }, []);

  useEffect(() => {
    setIsMobileMenuOpen(false);
  }, [pathname]);

  const handleLogout = () => {
    logout();
    setIsUserMenuOpen(false);
    setIsMobileMenuOpen(false);
    router.push("/login");
  };

  const userInitials = user?.name
    ? user.name
        .split(" ")
        .map((n) => n[0])
        .join("")
        .toUpperCase()
        .slice(0, 2)
    : user?.email?.slice(0, 2).toUpperCase() || "U";

  return (
    <header className="sticky top-0 z-40 w-full border-b bg-background/95 backdrop-blur supports-[backdrop-filter]:bg-background/60">
      <div className="flex h-16 items-center justify-between px-4 md:px-6">
        {/* Logo & Brand */}
        <div className="flex items-center gap-4">
          <Link href="/" className="flex items-center gap-2">
            <div className="flex h-8 w-8 items-center justify-center rounded-lg bg-primary text-primary-foreground font-bold">
              AB
            </div>
            <span className="hidden font-semibold sm:inline-block">AB-Bot</span>
          </Link>

          {/* Mode Indicator - Desktop */}
          {currentWorkspace && (
            <Badge
              variant="secondary"
              className="hidden md:flex items-center gap-1"
            >
              <ModeIcon className="h-3 w-3" />
              {modeLabel}
            </Badge>
          )}
        </div>

        {/* Mobile Menu Toggle */}
        <div className="md:hidden" ref={mobileMenuRef}>
          <Button
            variant="ghost"
            size="icon"
            onClick={() => setIsMobileMenuOpen(!isMobileMenuOpen)}
          >
            {isMobileMenuOpen ? (
              <X className="h-5 w-5" />
            ) : (
              <Menu className="h-5 w-5" />
            )}
          </Button>

          {/* Mobile Menu Dropdown */}
          {isMobileMenuOpen && (
            <div className="absolute left-0 right-0 top-16 bg-background border-b shadow-lg p-4 max-h-[calc(100vh-4rem)] overflow-y-auto">
              {mobileNavSections.map((section) => (
                <div key={section.title} className="mb-4">
                  <h3 className="text-xs font-semibold text-muted-foreground uppercase tracking-wider mb-2 px-3">
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
                          className={cn(
                            "flex items-center gap-3 rounded-lg px-3 py-2 text-sm font-medium transition-colors",
                            isActive
                              ? "bg-primary text-primary-foreground"
                              : "text-muted-foreground hover:bg-accent",
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

              {/* Mode Indicator in Mobile Menu */}
              {currentWorkspace && (
                <div className="pt-4 border-t">
                  <div className="flex items-center gap-2 px-3 py-2 text-sm text-muted-foreground">
                    <ModeIcon className="h-4 w-4" />
                    <span>{modeLabel} Mode</span>
                  </div>
                </div>
              )}
            </div>
          )}
        </div>

        {/* Wallet Info & Actions */}
        <div className="flex items-center gap-2">
          {/* Trading Pause/Resume */}
          {currentWorkspace && (
            <Button
              variant="ghost"
              size="sm"
              className={cn(
                "gap-1.5 text-xs font-medium",
                isTradingActive
                  ? "text-emerald-600 hover:text-amber-600 dark:text-emerald-400 dark:hover:text-amber-400"
                  : "text-amber-600 hover:text-emerald-600 dark:text-amber-400 dark:hover:text-emerald-400",
              )}
              disabled={toggleTradingMutation.isPending}
              onClick={() => toggleTradingMutation.mutate()}
            >
              {toggleTradingMutation.isPending ? (
                <span className="h-4 w-4 animate-spin rounded-full border-2 border-current border-t-transparent" />
              ) : isTradingActive ? (
                <Pause className="h-4 w-4" />
              ) : (
                <Play className="h-4 w-4" />
              )}
              <span className="hidden sm:inline">
                {isTradingActive ? "Pause" : "Resume"}
              </span>
            </Button>
          )}

          <ConnectionStatus status={wsStatus} />

          {/* Wallet Balance & Info */}
          {hasWallet && primaryWallet ? (
            <div className="flex items-center gap-1.5 px-3 py-1.5 rounded-full bg-muted text-sm">
              <Wallet className="h-4 w-4 text-muted-foreground" />
              {walletBalance != null ? (
                <span className="font-medium">
                  {new Intl.NumberFormat("en-US", {
                    style: "currency",
                    currency: "USD",
                    minimumFractionDigits: 2,
                    maximumFractionDigits: 2,
                  }).format(walletBalance.usdc_balance)}
                </span>
              ) : (
                <span className="text-xs text-muted-foreground">...</span>
              )}
              <span className="hidden sm:inline text-[10px] text-muted-foreground">
                USDC.e
              </span>
              <span className="hidden md:inline text-muted-foreground">
                &middot;
              </span>
              <span className="hidden md:inline font-mono text-xs text-muted-foreground">
                {primaryWallet.label ||
                  `${primaryWallet.address.slice(0, 6)}...${primaryWallet.address.slice(-4)}`}
              </span>
            </div>
          ) : (
            <button
              onClick={() => setShowConnectModal(true)}
              className="flex items-center gap-1.5 px-3 py-1.5 rounded-full bg-primary/10 text-primary border border-primary/20 hover:bg-primary/20 text-sm font-medium transition-colors"
            >
              <Plus className="h-4 w-4" />
              <span className="hidden sm:inline">Connect</span>
              <Wallet className="h-4 w-4 sm:hidden" />
            </button>
          )}
          <ConnectWalletModal
            open={showConnectModal}
            onOpenChange={setShowConnectModal}
          />

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
              <ChevronDown
                className={cn(
                  "h-4 w-4 transition-transform",
                  isUserMenuOpen && "rotate-180",
                )}
              />
            </Button>

            {isUserMenuOpen && (
              <div className="absolute right-0 mt-2 w-56 rounded-md border bg-popover p-1 shadow-lg">
                <div className="px-3 py-2 border-b mb-1">
                  <p className="text-sm font-medium">{user?.name || "User"}</p>
                  <p className="text-xs text-muted-foreground">{user?.email}</p>
                  <p className="text-xs text-muted-foreground capitalize mt-1">
                    Role: {currentWorkspace?.my_role || user?.role}
                  </p>
                </div>
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
