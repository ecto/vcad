/**
 * Input preferences dialog.
 *
 * Tabbed dialog containing keyboard and mouse customization. Replaces the
 * old "Mouse Controls" submenu in the View menu — both input surfaces
 * live here now so users have one place to look for "how do I rebind X".
 */

import { useState } from "react";
import * as Dialog from "@radix-ui/react-dialog";
import { X } from "@phosphor-icons/react/dist/ssr/X";
import { Keyboard } from "@phosphor-icons/react/dist/ssr/Keyboard";
import { Mouse } from "@phosphor-icons/react/dist/ssr/Mouse";
import { cn } from "@/lib/utils";
import { KeyboardPrefsPanel } from "./KeyboardPrefsPanel";
import { CameraSettingsPanel } from "./CameraSettingsPanel";

type Tab = "keyboard" | "mouse";

interface InputPreferencesDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** Which tab to show first. Defaults to keyboard. */
  initialTab?: Tab;
}

export function InputPreferencesDialog({
  open,
  onOpenChange,
  initialTab = "keyboard",
}: InputPreferencesDialogProps) {
  const [tab, setTab] = useState<Tab>(initialTab);

  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-50 bg-black/50 backdrop-blur-sm" />
        <Dialog.Content
          className={cn(
            "fixed left-1/2 top-1/2 z-50 -translate-x-1/2 -translate-y-1/2",
            "w-[640px] max-w-[92vw] h-[560px] max-h-[88vh]",
            "flex flex-col bg-surface shadow-2xl border border-border select-none",
            "focus:outline-none",
          )}
        >
          {/* Header — title + tabs + close */}
          <div
            data-tauri-drag-region=""
            className="flex items-center justify-between px-4 h-10 border-b border-border"
          >
            <Dialog.Title className="text-sm font-medium text-text">
              Input Preferences
            </Dialog.Title>
            <Dialog.Close className="p-1 text-text-muted hover:text-text">
              <X size={14} />
            </Dialog.Close>
          </div>
          {/* Hidden description for accessibility — Radix warns without one. */}
          <Dialog.Description className="sr-only">
            Customize keyboard shortcuts and mouse navigation bindings.
          </Dialog.Description>

          {/* Tab strip */}
          <div className="flex border-b border-border">
            <TabButton
              active={tab === "keyboard"}
              onClick={() => setTab("keyboard")}
              icon={<Keyboard size={13} />}
              label="Keyboard"
            />
            <TabButton
              active={tab === "mouse"}
              onClick={() => setTab("mouse")}
              icon={<Mouse size={13} />}
              label="Mouse"
            />
          </div>

          {/* Body */}
          <div className="flex-1 min-h-0 overflow-hidden">
            {tab === "keyboard" ? (
              <KeyboardPrefsPanel className="h-full" />
            ) : (
              <div className="p-4 overflow-y-auto h-full">
                <CameraSettingsPanel />
              </div>
            )}
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

interface TabButtonProps {
  active: boolean;
  onClick: () => void;
  icon: React.ReactNode;
  label: string;
}

function TabButton({ active, onClick, icon, label }: TabButtonProps) {
  return (
    <button
      onClick={onClick}
      className={cn(
        "flex items-center gap-1.5 px-4 h-9 text-xs",
        "border-b-2 -mb-px transition-colors",
        active
          ? "text-text border-brand"
          : "text-text-muted border-transparent hover:text-text",
      )}
    >
      {icon}
      {label}
    </button>
  );
}
