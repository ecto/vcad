import type { ComponentType } from "react";
import { Cube } from "@phosphor-icons/react/dist/ssr/Cube";
import { Cylinder } from "@phosphor-icons/react/dist/ssr/Cylinder";
import { Globe } from "@phosphor-icons/react/dist/ssr/Globe";
import { Unite } from "@phosphor-icons/react/dist/ssr/Unite";
import { Subtract } from "@phosphor-icons/react/dist/ssr/Subtract";
import { Intersect } from "@phosphor-icons/react/dist/ssr/Intersect";
import { ArrowsOutCardinal } from "@phosphor-icons/react/dist/ssr/ArrowsOutCardinal";
import { ArrowClockwise } from "@phosphor-icons/react/dist/ssr/ArrowClockwise";
import { ArrowCounterClockwise } from "@phosphor-icons/react/dist/ssr/ArrowCounterClockwise";
import { ArrowsOut } from "@phosphor-icons/react/dist/ssr/ArrowsOut";
import { ArrowsHorizontal } from "@phosphor-icons/react/dist/ssr/ArrowsHorizontal";
import { ArrowUp } from "@phosphor-icons/react/dist/ssr/ArrowUp";
import { ArrowRight } from "@phosphor-icons/react/dist/ssr/ArrowRight";
import { Trash } from "@phosphor-icons/react/dist/ssr/Trash";
import { Copy } from "@phosphor-icons/react/dist/ssr/Copy";
import { ClipboardText } from "@phosphor-icons/react/dist/ssr/ClipboardText";
import { Selection } from "@phosphor-icons/react/dist/ssr/Selection";
import { X } from "@phosphor-icons/react/dist/ssr/X";
import { CubeTransparent } from "@phosphor-icons/react/dist/ssr/CubeTransparent";
import { GridFour } from "@phosphor-icons/react/dist/ssr/GridFour";
import { SidebarSimple } from "@phosphor-icons/react/dist/ssr/SidebarSimple";
import { ChatDots } from "@phosphor-icons/react/dist/ssr/ChatDots";
import { Terminal } from "@phosphor-icons/react/dist/ssr/Terminal";
import { Sun } from "@phosphor-icons/react/dist/ssr/Sun";
import { Command } from "@phosphor-icons/react/dist/ssr/Command";
import { Pencil } from "@phosphor-icons/react/dist/ssr/Pencil";
import { Printer } from "@phosphor-icons/react/dist/ssr/Printer";
import { Wrench } from "@phosphor-icons/react/dist/ssr/Wrench";
import { FilePlus } from "@phosphor-icons/react/dist/ssr/FilePlus";
import { FolderOpen } from "@phosphor-icons/react/dist/ssr/FolderOpen";
import { FloppyDisk } from "@phosphor-icons/react/dist/ssr/FloppyDisk";
import { Export } from "@phosphor-icons/react/dist/ssr/Export";
import { CloudArrowDown } from "@phosphor-icons/react/dist/ssr/CloudArrowDown";
import { Info } from "@phosphor-icons/react/dist/ssr/Info";
import { Rocket } from "@phosphor-icons/react/dist/ssr/Rocket";
import { BookOpen } from "@phosphor-icons/react/dist/ssr/BookOpen";
import { GithubLogo } from "@phosphor-icons/react/dist/ssr/GithubLogo";
import { DiscordLogo } from "@phosphor-icons/react/dist/ssr/DiscordLogo";
import { Package } from "@phosphor-icons/react/dist/ssr/Package";
import { PlusSquare } from "@phosphor-icons/react/dist/ssr/PlusSquare";
import { Anchor } from "@phosphor-icons/react/dist/ssr/Anchor";
import { Circle } from "@phosphor-icons/react/dist/ssr/Circle";
import { Octagon } from "@phosphor-icons/react/dist/ssr/Octagon";
import { DotsThree } from "@phosphor-icons/react/dist/ssr/DotsThree";
import { CircleNotch } from "@phosphor-icons/react/dist/ssr/CircleNotch";
import { Scissors } from "@phosphor-icons/react/dist/ssr/Scissors";
import { Circuitry } from "@phosphor-icons/react/dist/ssr/Circuitry";
import { Sparkle } from "@phosphor-icons/react/dist/ssr/Sparkle";

type IconComponent = ComponentType<{ size?: number; weight?: "bold" | "regular" | "fill"; className?: string }>;

/** Maps the string `Command.icon` field onto a phosphor component. Keep in sync
 * with icon names used in @vcad/core/commands.ts and any app-layer additions. */
export const COMMAND_ICONS: Record<string, IconComponent> = {
  Cube,
  Cylinder,
  Globe,
  Unite,
  Subtract,
  Intersect,
  ArrowsOutCardinal,
  ArrowClockwise,
  ArrowCounterClockwise,
  ArrowsOut,
  ArrowsHorizontal,
  ArrowsClockwise: ArrowClockwise,
  ArrowUp,
  ArrowRight,
  Trash,
  Copy,
  ClipboardText,
  Selection,
  X,
  CubeTransparent,
  GridFour,
  SidebarSimple,
  ChatDots,
  Terminal,
  Sun,
  Command,
  Pencil,
  Printer,
  Wrench,
  FilePlus,
  FolderOpen,
  FloppyDisk,
  Export,
  CloudArrowDown,
  Info,
  Rocket,
  BookOpen,
  GithubLogo,
  DiscordLogo,
  Package,
  PlusSquare,
  Anchor,
  Circle,
  Octagon,
  DotsThree,
  CircleNotch,
  Scissors,
  CircuitBoard: Circuitry,
  Circuitry,
  Sparkle,
};

export type { IconComponent };
