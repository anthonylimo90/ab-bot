"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";
import {
  Star,
} from "lucide-react";
import { cn } from "@/lib/utils";
import { useWorkspaceStore } from "@/stores/workspace-store";
import { useAllocationsQuery } from "@/hooks/queries/useAllocationsQuery";
import { primaryNavSections } from "@/components/layout/navigation";

export function Sidebar() {
  const pathname = usePathname();
  const { currentWorkspace } = useWorkspaceStore();
  const { data: allocations = [] } = useAllocationsQuery(currentWorkspace?.id);
  const activeWallets = allocations.filter((a) => a.tier === "active");
  const benchWallets = allocations.filter((a) => a.tier === "bench");

  const getBadgeCount = (badgeType?: "active" | "watching") => {
    if (badgeType === "active") return activeWallets.length;
    if (badgeType === "watching") return benchWallets.length;
    return null;
  };

  return (
    <aside className="fixed left-0 top-16 z-30 hidden h-[calc(100vh-4rem)] w-64 border-r bg-background md:block">
      <nav className="flex flex-col gap-6 p-4">
        {primaryNavSections.map((section) => (
          <div key={section.title} className="space-y-1">
            <h3 className="px-3 text-xs font-semibold text-muted-foreground uppercase tracking-wider">
              {section.title}
            </h3>
            <div className="space-y-1">
              {section.items.map((item) => {
                const isActive =
                  pathname === item.href ||
                  (item.href !== "/" && pathname.startsWith(item.href));
                const Icon = item.icon;
                const badgeCount = getBadgeCount(item.badge);

                return (
                  <Link
                    key={item.href}
                    href={item.href}
                    className={cn(
                      "flex items-center justify-between rounded-lg px-3 py-2 text-sm font-medium transition-colors",
                      isActive
                        ? "bg-primary text-primary-foreground"
                        : "text-muted-foreground hover:bg-accent hover:text-accent-foreground",
                    )}
                  >
                    <div className="flex items-center gap-3">
                      <Icon className="h-4 w-4" />
                      {item.label}
                    </div>
                    {badgeCount !== null && badgeCount > 0 && (
                      <span
                        className={cn(
                          "flex h-5 min-w-5 items-center justify-center rounded-full px-1.5 text-xs font-medium",
                          isActive
                            ? "bg-primary-foreground/20 text-primary-foreground"
                            : "bg-muted text-muted-foreground",
                        )}
                      >
                        {badgeCount}
                      </span>
                    )}
                  </Link>
                );
              })}
            </div>
          </div>
        ))}
      </nav>

      {/* Active Wallets Summary */}
      <div className="absolute bottom-4 left-4 right-4">
        <div className="rounded-lg border bg-muted/30 p-3 space-y-2">
          <div className="flex items-center justify-between text-xs">
            <span className="text-muted-foreground flex items-center gap-1.5">
              <Star className="h-3 w-3" />
              Active Wallets
            </span>
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
              {5 - activeWallets.length} slot
              {5 - activeWallets.length !== 1 ? "s" : ""} available
            </p>
          )}
        </div>
      </div>
    </aside>
  );
}
