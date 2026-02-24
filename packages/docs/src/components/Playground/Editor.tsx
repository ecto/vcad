"use client";

import { useRef, useEffect } from "react";
import MonacoEditor, { type OnMount, type Monaco } from "@monaco-editor/react";
import type { editor } from "monaco-editor";
import { useTheme } from "@/components/ThemeProvider";

interface EditorProps {
  value: string;
  onChange: (value: string) => void;
  language?: string;
  readOnly?: boolean;
}

// Monokai-inspired theme matching vcad.io
const darkTheme: editor.IStandaloneThemeData = {
  base: "vs-dark",
  inherit: false,
  rules: [
    { token: "", foreground: "d4d4d8", background: "09090b" },
    { token: "comment", foreground: "71717a", fontStyle: "italic" },
    { token: "keyword", foreground: "F92672" },
    { token: "string", foreground: "A6E22E" },
    { token: "number", foreground: "AE81FF" },
    { token: "type", foreground: "ffc66d" },
    { token: "function", foreground: "ffc66d" },
    { token: "variable", foreground: "d4d4d8" },
    { token: "operator", foreground: "d4d4d8" },
    { token: "delimiter", foreground: "d4d4d8" },
  ],
  colors: {
    "editor.background": "#18181b",
    "editor.foreground": "#d4d4d8",
    "editor.lineHighlightBackground": "#27272a",
    "editor.selectionBackground": "#F9267240",
    "editor.inactiveSelectionBackground": "#F9267220",
    "editorCursor.foreground": "#F92672",
    "editorLineNumber.foreground": "#3f3f46",
    "editorLineNumber.activeForeground": "#71717a",
    "editorIndentGuide.background": "#27272a",
    "editorIndentGuide.activeBackground": "#3f3f46",
    "editorWhitespace.foreground": "#27272a",
    "scrollbarSlider.background": "#27272a",
    "scrollbarSlider.hoverBackground": "#3f3f46",
    "scrollbarSlider.activeBackground": "#52525b",
  },
};

const lightTheme: editor.IStandaloneThemeData = {
  base: "vs",
  inherit: false,
  rules: [
    { token: "", foreground: "27272a", background: "f4f4f5" },
    { token: "comment", foreground: "71717a", fontStyle: "italic" },
    { token: "keyword", foreground: "9d3a0a" },
    { token: "string", foreground: "2a6f2d" },
    { token: "number", foreground: "1558b0" },
    { token: "type", foreground: "9d3a0a" },
    { token: "function", foreground: "9d3a0a" },
    { token: "variable", foreground: "27272a" },
    { token: "operator", foreground: "27272a" },
    { token: "delimiter", foreground: "27272a" },
  ],
  colors: {
    "editor.background": "#f4f4f5",
    "editor.foreground": "#27272a",
    "editor.lineHighlightBackground": "#e4e4e7",
    "editor.selectionBackground": "#F9267240",
    "editor.inactiveSelectionBackground": "#F9267220",
    "editorCursor.foreground": "#F92672",
    "editorLineNumber.foreground": "#a1a1aa",
    "editorLineNumber.activeForeground": "#71717a",
    "editorIndentGuide.background": "#e4e4e7",
    "editorIndentGuide.activeBackground": "#d4d4d8",
    "editorWhitespace.foreground": "#d4d4d8",
    "scrollbarSlider.background": "#e4e4e7",
    "scrollbarSlider.hoverBackground": "#d4d4d8",
    "scrollbarSlider.activeBackground": "#a1a1aa",
  },
};

export function Editor({ value, onChange, language = "rust", readOnly = false }: EditorProps) {
  const { theme } = useTheme();
  const editorRef = useRef<editor.IStandaloneCodeEditor | null>(null);
  const monacoRef = useRef<Monaco | null>(null);

  const handleEditorDidMount: OnMount = (editor, monaco) => {
    editorRef.current = editor;
    monacoRef.current = monaco;

    // Register custom themes
    monaco.editor.defineTheme("vcad-dark", darkTheme);
    monaco.editor.defineTheme("vcad-light", lightTheme);
    monaco.editor.setTheme(theme === "dark" ? "vcad-dark" : "vcad-light");

    // Set editor options
    editor.updateOptions({
      fontSize: 13,
      fontFamily: "'Berkeley Mono', 'SF Mono', ui-monospace, monospace",
      lineHeight: 22,
      minimap: { enabled: false },
      scrollBeyondLastLine: false,
      renderLineHighlight: "line",
      cursorBlinking: "smooth",
      cursorSmoothCaretAnimation: "on",
      smoothScrolling: true,
      padding: { top: 16, bottom: 16 },
      lineNumbers: "on",
      glyphMargin: false,
      folding: false,
      lineDecorationsWidth: 8,
      lineNumbersMinChars: 3,
      readOnly,
    });
  };

  // Update theme when it changes
  useEffect(() => {
    if (monacoRef.current) {
      monacoRef.current.editor.setTheme(theme === "dark" ? "vcad-dark" : "vcad-light");
    }
  }, [theme]);

  return (
    <div className="h-full min-h-[300px] rounded-lg border border-border overflow-hidden">
      <MonacoEditor
        height="100%"
        language={language}
        value={value}
        onChange={(v) => onChange(v ?? "")}
        onMount={handleEditorDidMount}
        theme={theme === "dark" ? "vcad-dark" : "vcad-light"}
        options={{
          readOnly,
          automaticLayout: true,
        }}
        loading={
          <div className="h-full flex items-center justify-center text-text-muted text-sm">
            Loading editor...
          </div>
        }
      />
    </div>
  );
}
