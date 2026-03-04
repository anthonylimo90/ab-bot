import {
  LayoutDashboard,
  Settings,
  LineChart,
  History,
  ShieldAlert,
  BarChart2,
  Zap,
  Activity,
  Layers,
  SlidersHorizontal,
  type LucideIcon,
} from "lucide-react";

export interface NavItem {
  href: string;
  label: string;
  icon: LucideIcon;
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
      { href: "/markets", label: "Markets", icon: BarChart2 },
      { href: "/positions", label: "Positions", icon: Layers },
      { href: "/signals", label: "Quant Signals", icon: Zap },
      { href: "/activity", label: "Activity", icon: Activity },
      { href: "/backtest", label: "Backtest", icon: LineChart },
      { href: "/history", label: "History", icon: History },
      { href: "/risk", label: "Risk Monitor", icon: ShieldAlert },
      { href: "/tuner", label: "Tuner", icon: SlidersHorizontal },
    ],
  },
  {
    title: "Settings",
    items: [{ href: "/settings", label: "Settings", icon: Settings }],
  },
];
