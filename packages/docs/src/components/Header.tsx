"use client";

import { useState } from "react";
import Link from "next/link";
import { usePathname } from "next/navigation";
import { List, X, Sun, Moon } from "@phosphor-icons/react";
import { useTheme } from "./ThemeProvider";
import { cn } from "@/lib/utils";

const navLinks = [
  { label: "Features", href: "/#features" },
  { label: "Docs", href: "/" },
  { label: "Source", href: "https://github.com/ecto/vcad", external: true },
];

export function Header() {
  const pathname = usePathname();
  const { theme, toggleTheme } = useTheme();
  const [mobileOpen, setMobileOpen] = useState(false);

  const isDocsPage = true;

  return (
    <>
      <header className="sticky top-0 z-50 h-14 bg-bg/95 backdrop-blur border-b border-border">
        <div className={cn(
          "h-full flex items-center justify-between",
          isDocsPage ? "px-4" : "max-w-[720px] mx-auto px-8"
        )}>
          {/* Logo */}
          <Link href="/" className="font-bold text-lg">
            vcad<span className="text-accent">.</span>
          </Link>

          {/* Desktop nav */}
          <nav className="hidden sm:flex items-center gap-6">
            {navLinks.map((link) => (
              <Link
                key={link.href}
                href={link.href}
                className={cn(
                  "text-sm text-text-muted hover:text-text transition-colors",
                  pathname === link.href && "text-text"
                )}
                {...(link.external ? { target: "_blank", rel: "noopener noreferrer" } : {})}
              >
                {link.label.toLowerCase()}
              </Link>
            ))}
            <button
              onClick={toggleTheme}
              className="p-2 hover:bg-hover rounded-md transition-colors text-text-muted hover:text-text"
              aria-label="Toggle theme"
            >
              {theme === "dark" ? <Sun size={18} /> : <Moon size={18} />}
            </button>
          </nav>

          {/* Mobile menu button */}
          <div className="flex sm:hidden items-center gap-2">
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
        </div>
      </header>

      {/* Mobile menu overlay */}
      {mobileOpen && (
        <div
          className="sm:hidden fixed inset-0 z-40 bg-black/50"
          onClick={() => setMobileOpen(false)}
        />
      )}

      {/* Mobile menu */}
      {mobileOpen && (
        <div className="sm:hidden fixed top-14 left-0 right-0 z-50 bg-bg border-b border-border">
          <nav className="flex flex-col p-4 gap-2">
            {navLinks.map((link) => (
              <Link
                key={link.href}
                href={link.href}
                onClick={() => setMobileOpen(false)}
                className="px-4 py-2 text-text-muted hover:text-text hover:bg-hover rounded-md transition-colors"
                {...(link.external ? { target: "_blank", rel: "noopener noreferrer" } : {})}
              >
                {link.label.toLowerCase()}
              </Link>
            ))}
          </nav>
        </div>
      )}
    </>
  );
}
