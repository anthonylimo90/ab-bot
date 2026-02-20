import {
  LayoutDashboard,
  Search,
  TrendingUp,
  Settings,
  LineChart,
  History,
  ShieldAlert,
  BarChart2,
  Zap,
  type LucideIcon,
} from "lucide-react";

export interface NavItem {
  href: string;
  label: string;
  icon: LucideIcon;
  badge?: "active" | "watching";
}

export interface NavSection {
  title: string;
  items: NavItem[];
}

export const primaryNavSections: NavSection[] = [
  {
    title: "Overview",
    items: [{ href: "/", label: "Dashboard", icon: LayoutDashboard }],
  },
  {
    title: "Trading",
    items: [
      { href: "/discover", label: "Discover", icon: Search },
      { href: "/trading", label: "Trading", icon: TrendingUp, badge: "active" },
      { href: "/markets", label: "Markets", icon: BarChart2 },
      { href: "/signals", label: "Signals", icon: Zap },
      { href: "/backtest", label: "Backtest", icon: LineChart },
      { href: "/history", label: "History", icon: History },
      { href: "/risk", label: "Risk Monitor", icon: ShieldAlert },
    ],
  },
  {
    title: "Settings",
    items: [{ href: "/settings", label: "Settings", icon: Settings }],
  },
];
