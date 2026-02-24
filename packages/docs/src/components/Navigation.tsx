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
  List,
  X,
  CaretRight,
  GameController,
  Code,
  Compass,
  Wrench,
  Scales,
  RocketLaunch,
} from "@phosphor-icons/react";
import { useTheme } from "./ThemeProvider";
import { useSearch } from "./Search/SearchProvider";
import { cn } from "@/lib/utils";

interface NavChild {
  label: string;
  href: string;
}

interface NavItem {
  label: string;
  href: string;
  icon: React.ReactNode;
  children?: NavChild[];
}

const navItems: NavItem[] = [
  {
    label: "Home",
    href: "/",
    icon: <House size={18} weight="regular" />,
  },
  {
    label: "Get Started",
    href: "/start",
    icon: <RocketLaunch size={18} weight="regular" />,
    children: [
      { label: "Quick Start", href: "/start/quick-start" },
      { label: "Install & Setup", href: "/start/install" },
      { label: "Core Concepts", href: "/start/concepts" },
    ],
  },
  {
    label: "Tutorials",
    href: "/tutorials",
    icon: <Book size={18} weight="regular" />,
    children: [
      { label: "App Tutorials", href: "/tutorials/app" },
      { label: "Rust Tutorials", href: "/tutorials/rust" },
      { label: "CLI Tutorials", href: "/tutorials/cli" },
      { label: "MCP / AI Tutorials", href: "/tutorials/mcp" },
    ],
  },
  {
    label: "Guides",
    href: "/guides",
    icon: <Compass size={18} weight="regular" />,
    children: [
      { label: "Modeling", href: "/guides/modeling" },
      { label: "Assembly & Motion", href: "/guides/assembly" },
      { label: "Manufacturing", href: "/guides/mfg" },
      { label: "Electronics", href: "/guides/electronics" },
      { label: "AI & Automation", href: "/guides/ai" },
    ],
  },
  {
    label: "Reference",
    href: "/reference",
    icon: <Code size={18} weight="regular" />,
    children: [
      { label: "App", href: "/reference/app" },
      { label: "Rust API", href: "/reference/rust" },
      { label: "CLI", href: "/reference/cli" },
      { label: "MCP Tools", href: "/reference/mcp" },
      { label: "IR & Format", href: "/reference/format" },
      { label: "Loon Language", href: "/reference/loon" },
    ],
  },
  {
    label: "Cookbook",
    href: "/cookbook",
    icon: <CookingPot size={18} weight="regular" />,
  },
  {
    label: "Architecture",
    href: "/architecture",
    icon: <Cpu size={18} weight="regular" />,
  },
  {
    label: "Playground",
    href: "/playground",
    icon: <GameController size={18} weight="regular" />,
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
      { label: "vs Onshape", href: "/vs/onshape" },
      { label: "vs Fusion 360", href: "/vs/fusion360" },
      { label: "vs OpenSCAD", href: "/vs/openscad" },
      { label: "vs FreeCAD", href: "/vs/freecad" },
      { label: "vs CadQuery", href: "/vs/cadquery" },
    ],
  },
];

export function Navigation() {
  const pathname = usePathname();
  const { theme, toggleTheme } = useTheme();
  const { openSearch } = useSearch();
  const [mobileOpen, setMobileOpen] = useState(false);
  const [expandedSections, setExpandedSections] = useState<Set<string>>(
    () => {
      // Auto-expand the section matching the current path
      const initial = new Set<string>();
      for (const item of navItems) {
        if (item.children && pathname.startsWith(item.href) && item.href !== "/") {
          initial.add(item.label);
        }
      }
      return initial;
    }
  );

  const toggleSection = (label: string) => {
    setExpandedSections(prev => {
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
            {theme === "dark" ? <Sun size={20} /> : <Moon size={20} />}
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
          <Link href="/" className="font-bold text-lg font-mono" onClick={() => setMobileOpen(false)}>
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
          <ul className="space-y-1">
            {navItems.map((item) => (
              <li key={item.href}>
                {item.children ? (
                  <div>
                    <button
                      onClick={() => toggleSection(item.label)}
                      className={cn(
                        "w-full flex items-center gap-2 px-3 py-2 rounded-md text-sm transition-colors",
                        isActive(item.href)
                          ? "bg-accent/10 text-accent"
                          : "text-text-muted hover:text-text hover:bg-hover"
                      )}
                    >
                      {item.icon}
                      <span className="flex-1 text-left font-mono">{item.label}</span>
                      <CaretRight
                        size={14}
                        className={cn(
                          "transition-transform",
                          expandedSections.has(item.label) && "rotate-90"
                        )}
                      />
                    </button>
                    {expandedSections.has(item.label) && (
                      <ul className="mt-1 ml-7 space-y-1">
                        {item.children.map((child) => (
                          <li key={child.href}>
                            <Link
                              href={child.href}
                              onClick={() => setMobileOpen(false)}
                              className={cn(
                                "block px-3 py-1.5 rounded-md text-sm transition-colors",
                                pathname === child.href || pathname.startsWith(child.href + "/")
                                  ? "bg-accent/10 text-accent"
                                  : "text-text-muted hover:text-text hover:bg-hover"
                              )}
                            >
                              {child.label}
                            </Link>
                          </li>
                        ))}
                      </ul>
                    )}
                  </div>
                ) : (
                  <Link
                    href={item.href}
                    onClick={() => setMobileOpen(false)}
                    className={cn(
                      "flex items-center gap-2 px-3 py-2 rounded-md text-sm transition-colors",
                      isActive(item.href)
                        ? "bg-accent/10 text-accent"
                        : "text-text-muted hover:text-text hover:bg-hover"
                    )}
                  >
                    {item.icon}
                    <span className="font-mono">{item.label}</span>
                  </Link>
                )}
              </li>
            ))}
          </ul>
        </nav>

        {/* Footer */}
        <div className="px-3 py-3 border-t border-border">
          <div className="flex items-center justify-between">
            <div className="flex gap-2 text-xs text-text-muted">
              <a
                href="https://github.com/ecto/vcad"
                className="hover:text-text transition-colors"
                target="_blank"
                rel="noopener noreferrer"
              >
                github
              </a>
              <span>·</span>
              <a
                href="https://vcad.io"
                className="hover:text-text transition-colors"
                target="_blank"
                rel="noopener noreferrer"
              >
                vcad.io
              </a>
              <span>·</span>
              <a
                href="https://crates.io/crates/vcad"
                className="hover:text-text transition-colors"
                target="_blank"
                rel="noopener noreferrer"
              >
                crates.io
              </a>
            </div>
            <button
              onClick={toggleTheme}
              className="p-2 hover:bg-hover rounded-md transition-colors"
              aria-label="Toggle theme"
            >
              {theme === "dark" ? <Sun size={16} /> : <Moon size={16} />}
            </button>
          </div>
        </div>
      </aside>

      {/* Main content padding for mobile header */}
      <div className="lg:hidden h-14" />
    </>
  );
}
