"use client";

import { useState } from "react";
import Link from "next/link";
import { usePathname } from "next/navigation";
import {
  House,
  Book,
  CookingPot,
  Images,
  Cpu,
  MagnifyingGlass,
  Sun,
  Moon,
  Desktop,
  List,
  X,
  CaretRight,
  GameController,
  Code,
  Compass,
  Wrench,
  Scales,
  RocketLaunch,
  GithubLogo,
  Package,
  Globe,
  Lightning,
  DownloadSimple,
  Lightbulb,
  AppWindow,
  GearSix,
  Terminal,
  Robot,
  Cube,
  Link as LinkIcon,
  Factory,
  BracketsCurly,
  Scroll,
  Plugs,
  Brain,
  ArrowsLeftRight,
} from "@phosphor-icons/react";
import { useTheme } from "./ThemeProvider";
import { useSearch } from "./Search/SearchProvider";
import { cn } from "@/lib/utils";

interface NavChild {
  label: string;
  href: string;
  icon: React.ReactNode;
}

interface NavItem {
  label: string;
  href: string;
  icon: React.ReactNode;
  children?: NavChild[];
}

interface NavGroup {
  label?: string;
  items: NavItem[];
}

const navGroups: NavGroup[] = [
  {
    items: [
      {
        label: "Home",
        href: "/",
        icon: <House size={18} weight="regular" />,
      },
    ],
  },
  {
    label: "Learn",
    items: [
      {
        label: "Get Started",
        href: "/start",
        icon: <RocketLaunch size={18} weight="regular" />,
        children: [
          { label: "Quick Start", href: "/start/quick-start", icon: <Lightning size={14} /> },
          { label: "Install & Setup", href: "/start/install", icon: <DownloadSimple size={14} /> },
          { label: "Core Concepts", href: "/start/concepts", icon: <Lightbulb size={14} /> },
        ],
      },
      {
        label: "Tutorials",
        href: "/tutorials",
        icon: <Book size={18} weight="regular" />,
        children: [
          { label: "App Tutorials", href: "/tutorials/app", icon: <AppWindow size={14} /> },
          { label: "Rust Tutorials", href: "/tutorials/rust", icon: <GearSix size={14} /> },
          { label: "CLI Tutorials", href: "/tutorials/cli", icon: <Terminal size={14} /> },
          { label: "MCP / AI Tutorials", href: "/tutorials/mcp", icon: <Robot size={14} /> },
        ],
      },
      {
        label: "Guides",
        href: "/guides",
        icon: <Compass size={18} weight="regular" />,
        children: [
          { label: "Modeling", href: "/guides/modeling", icon: <Cube size={14} /> },
          { label: "Assembly & Motion", href: "/guides/assembly", icon: <LinkIcon size={14} /> },
          { label: "Manufacturing", href: "/guides/mfg", icon: <Factory size={14} /> },
          { label: "Electronics", href: "/guides/electronics", icon: <Cpu size={14} /> },
          { label: "AI & Automation", href: "/guides/ai", icon: <Brain size={14} /> },
        ],
      },
    ],
  },
  {
    label: "Build",
    items: [
      {
        label: "Reference",
        href: "/reference",
        icon: <Code size={18} weight="regular" />,
        children: [
          { label: "App", href: "/reference/app", icon: <AppWindow size={14} /> },
          { label: "Rust API", href: "/reference/rust", icon: <GearSix size={14} /> },
          { label: "CLI", href: "/reference/cli", icon: <Terminal size={14} /> },
          { label: "MCP Tools", href: "/reference/mcp", icon: <Plugs size={14} /> },
          { label: "IR & Format", href: "/reference/format", icon: <BracketsCurly size={14} /> },
          { label: "Loon Language", href: "/reference/loon", icon: <Scroll size={14} /> },
        ],
      },
      {
        label: "Cookbook",
        href: "/cookbook",
        icon: <CookingPot size={18} weight="regular" />,
      },
      {
        label: "Playground",
        href: "/playground",
        icon: <GameController size={18} weight="regular" />,
      },
    ],
  },
  {
    label: "Explore",
    items: [
      {
        label: "Architecture",
        href: "/architecture",
        icon: <Cpu size={18} weight="regular" />,
      },
      {
        label: "Gallery",
        href: "/gallery",
        icon: <Images size={18} weight="regular" />,
      },
      {
        label: "Comparisons",
        href: "/vs",
        icon: <Scales size={18} weight="regular" />,
        children: [
          { label: "vs Onshape", href: "/vs/onshape", icon: <ArrowsLeftRight size={14} /> },
          { label: "vs Fusion 360", href: "/vs/fusion360", icon: <ArrowsLeftRight size={14} /> },
          { label: "vs OpenSCAD", href: "/vs/openscad", icon: <ArrowsLeftRight size={14} /> },
          { label: "vs FreeCAD", href: "/vs/freecad", icon: <ArrowsLeftRight size={14} /> },
          { label: "vs CadQuery", href: "/vs/cadquery", icon: <ArrowsLeftRight size={14} /> },
        ],
      },
    ],
  },
];

// Flat list for auto-expand logic
const allNavItems = navGroups.flatMap((g) => g.items);

export function Navigation() {
  const pathname = usePathname();
  const { theme, setting, toggleTheme } = useTheme();
  const { openSearch } = useSearch();
  const [mobileOpen, setMobileOpen] = useState(false);
  const [expandedSections, setExpandedSections] = useState<Set<string>>(
    () => new Set(allNavItems.filter((item) => item.children).map((item) => item.label))
  );

  const toggleSection = (label: string) => {
    setExpandedSections((prev) => {
      const next = new Set(prev);
      if (next.has(label)) {
        next.delete(label);
      } else {
        next.add(label);
      }
      return next;
    });
  };

  const isActive = (href: string) => {
    if (href === "/") return pathname === "/";
    return pathname.startsWith(href);
  };

  const renderNavItem = (item: NavItem) => {
    if (item.children) {
      const active = isActive(item.href);
      const expanded = expandedSections.has(item.label);
      return (
        <div key={item.href}>
          <button
            onClick={() => toggleSection(item.label)}
            className={cn(
              "w-full flex items-center gap-2 px-3 py-2 text-sm transition-colors border-l-2",
              active
                ? "border-accent text-text"
                : "border-transparent text-text-muted hover:text-accent"
            )}
          >
            {item.icon}
            <span className="flex-1 text-left font-mono">{item.label}</span>
            <CaretRight
              size={14}
              className={cn(
                "transition-transform text-text-muted",
                expanded && "rotate-90"
              )}
            />
          </button>
          {expanded && (
            <ul className="ml-[21px] border-l border-border">
              {item.children.map((child) => {
                const childActive =
                  pathname === child.href ||
                  pathname.startsWith(child.href + "/");
                return (
                  <li key={child.href}>
                    <Link
                      href={child.href}
                      onClick={() => setMobileOpen(false)}
                      className={cn(
                        "flex items-center gap-2 px-3 py-1.5 text-[13px] transition-colors border-l-2 -ml-px",
                        childActive
                          ? "border-accent text-text"
                          : "border-transparent text-text-muted hover:text-accent"
                      )}
                    >
                      {child.icon}
                      {child.label}
                    </Link>
                  </li>
                );
              })}
            </ul>
          )}
        </div>
      );
    }

    return (
      <Link
        key={item.href}
        href={item.href}
        onClick={() => setMobileOpen(false)}
        className={cn(
          "flex items-center gap-2 px-3 py-2 text-sm transition-colors border-l-2",
          isActive(item.href)
            ? "border-accent text-text"
            : "border-transparent text-text-muted hover:text-accent"
        )}
      >
        {item.icon}
        <span className="font-mono">{item.label}</span>
      </Link>
    );
  };

  return (
    <>
      {/* Mobile header */}
      <header className="lg:hidden fixed top-0 left-0 right-0 z-50 h-14 bg-bg/95 backdrop-blur border-b border-border flex items-center justify-between px-4">
        <Link href="/" className="font-bold text-lg font-mono">
          vcad<span className="text-accent">.</span>
        </Link>
        <div className="flex items-center gap-2">
          <button
            onClick={openSearch}
            className="p-2 hover:bg-hover rounded-md transition-colors"
            aria-label="Search"
          >
            <MagnifyingGlass size={20} />
          </button>
          <button
            onClick={toggleTheme}
            className="p-2 hover:bg-hover rounded-md transition-colors"
            aria-label="Toggle theme"
          >
            {setting === "system" ? (
              <Desktop size={20} />
            ) : theme === "dark" ? (
              <Sun size={20} />
            ) : (
              <Moon size={20} />
            )}
          </button>
          <button
            onClick={() => setMobileOpen(!mobileOpen)}
            className="p-2 hover:bg-hover rounded-md transition-colors"
            aria-label="Toggle menu"
          >
            {mobileOpen ? <X size={20} /> : <List size={20} />}
          </button>
        </div>
      </header>

      {/* Mobile menu overlay */}
      {mobileOpen && (
        <div
          className="lg:hidden fixed inset-0 z-40 bg-black/50"
          onClick={() => setMobileOpen(false)}
        />
      )}

      {/* Sidebar */}
      <aside
        className={cn(
          "fixed lg:sticky top-0 left-0 z-50 lg:z-0",
          "h-screen w-64 bg-surface border-r border-border",
          "flex flex-col overflow-hidden",
          "transition-transform lg:translate-x-0",
          mobileOpen ? "translate-x-0" : "-translate-x-full"
        )}
      >
        {/* Logo */}
        <div className="h-14 flex items-center px-4 border-b border-border-subtle">
          <Link
            href="/"
            className="font-bold text-lg font-mono"
            onClick={() => setMobileOpen(false)}
          >
            vcad<span className="text-accent">.</span>
          </Link>
        </div>

        {/* Search button */}
        <div className="px-3 py-3">
          <button
            onClick={openSearch}
            className="w-full flex items-center gap-2 px-3 py-2 text-sm text-text-muted bg-surface hover:bg-hover rounded-md border border-border transition-colors"
          >
            <MagnifyingGlass size={16} />
            <span className="flex-1 text-left">Search...</span>
            <kbd className="text-xs px-1.5 py-0.5 bg-bg rounded border border-border">
              ⌘K
            </kbd>
          </button>
        </div>

        {/* Nav items */}
        <nav className="flex-1 overflow-y-auto px-3 py-2">
          {navGroups.map((group, gi) => (
            <div key={gi}>
              {group.label && (
                <div className="mt-6 mb-2 px-3 text-[11px] font-mono uppercase tracking-widest text-text-muted select-none">
                  {group.label}
                </div>
              )}
              <ul className="space-y-0.5">
                {group.items.map((item) => (
                  <li key={item.href}>{renderNavItem(item)}</li>
                ))}
              </ul>
            </div>
          ))}
        </nav>

        {/* Stats card */}
        <div className="mt-auto mx-3 mb-3 rounded-lg border border-border-subtle bg-bg p-3">
          <div className="text-xs font-mono text-text-muted leading-relaxed">
            <span>155+ pages</span>
            <span className="mx-1.5 opacity-40">·</span>
            <span>12 topics</span>
          </div>
          <div className="mt-1.5 text-xs font-mono text-text-muted/60 leading-relaxed">
            brep kernel · web app · rust cli · mcp server
          </div>
        </div>

        {/* Footer */}
        <div className="px-3 py-3 border-t border-border">
          <div className="text-xs font-mono text-text-muted text-center mb-2">
            v0.8.0 · MIT
          </div>
          <div className="flex items-center justify-center gap-1">
            <a
              href="https://github.com/ecto/vcad"
              className="p-2 text-text-muted hover:text-text rounded-md transition-colors"
              target="_blank"
              rel="noopener noreferrer"
              aria-label="GitHub"
            >
              <GithubLogo size={16} />
            </a>
            <a
              href="https://www.npmjs.com/org/vcad"
              className="p-2 text-text-muted hover:text-text rounded-md transition-colors"
              target="_blank"
              rel="noopener noreferrer"
              aria-label="npm"
            >
              <Package size={16} />
            </a>
            <a
              href="https://vcad.io"
              className="p-2 text-text-muted hover:text-text rounded-md transition-colors"
              target="_blank"
              rel="noopener noreferrer"
              aria-label="vcad.io"
            >
              <Globe size={16} />
            </a>
            <button
              onClick={toggleTheme}
              className="p-2 text-text-muted hover:text-text rounded-md transition-colors"
              aria-label="Toggle theme"
            >
              {setting === "system" ? (
                <Desktop size={16} />
              ) : theme === "dark" ? (
                <Sun size={16} />
              ) : (
                <Moon size={16} />
              )}
            </button>
          </div>
        </div>
      </aside>

      {/* Main content padding for mobile header */}
      <div className="lg:hidden h-14" />
    </>
  );
}
