/**
 * Unified logging system for vcad.
 *
 * Provides structured logging with levels, sources, and subscription support.
 * Intercepts console calls with recognized prefixes (e.g., [WASM], [STEP]).
 */

export const LogLevel = {
  DEBUG: 0,
  INFO: 1,
  WARN: 2,
  ERROR: 3,
} as const;

export type LogLevelName = keyof typeof LogLevel;

export const LogSource = {
  KERNEL: "kernel",
  ENGINE: "engine",
  APP: "app",
  GPU: "gpu",
  STEP: "step",
  MESH: "mesh",
  WASM: "wasm",
} as const;

export type LogSourceName = (typeof LogSource)[keyof typeof LogSource];

export interface LogEntry {
  id: number;
  timestamp: number;
  level: LogLevelName;
  source: LogSourceName;
  message: string;
}

export type LogSubscriber = (entry: LogEntry) => void;

/** Map console prefixes to sources */
const PREFIX_TO_SOURCE: Record<string, LogSourceName> = {
  "[WASM]": "kernel",
  "[KERNEL]": "kernel",
  "[ENGINE]": "engine",
  "[APP]": "app",
  "[GPU]": "gpu",
  "[STEP]": "step",
  "[MESH]": "mesh",
};

const PREFIX_REGEX = /^\[(WASM|KERNEL|ENGINE|APP|GPU|STEP|MESH)\]\s*/i;

class Logger {
  private entries: LogEntry[] = [];
  private maxEntries = 1000;
  private nextId = 1;
  private subscribers = new Set<LogSubscriber>();
  private intercepted = false;

  // Store original console methods
  private originalConsole = {
    log: console.log.bind(console),
    info: console.info.bind(console),
    warn: console.warn.bind(console),
    error: console.error.bind(console),
    debug: console.debug.bind(console),
  };

  constructor() {
    // Auto-intercept in browser environment
    if (typeof window !== "undefined") {
      this.interceptConsole();
    }
  }

  /**
   * Intercept console calls to capture logs with recognized prefixes.
   */
  interceptConsole(): void {
    if (this.intercepted) return;
    this.intercepted = true;

    const self = this;

    const createInterceptor = (
      original: (...args: unknown[]) => void,
      level: LogLevelName,
    ) => {
      return function (...args: unknown[]) {
        // Always call original console
        original(...args);

        // Try to extract source from prefix
        if (args.length > 0 && typeof args[0] === "string") {
          const match = args[0].match(PREFIX_REGEX);
          if (match) {
            const prefix = `[${match[1]!.toUpperCase()}]`;
            const source = PREFIX_TO_SOURCE[prefix];
            if (source) {
              // Remove prefix from message
              const message = args
                .map((a, i) =>
                  i === 0 ? String(a).replace(PREFIX_REGEX, "") : String(a),
                )
                .join(" ");
              self.addEntry(level, source, message);
            }
          }
        }
      };
    };

    console.log = createInterceptor(this.originalConsole.log, "INFO");
    console.info = createInterceptor(this.originalConsole.info, "INFO");
    console.warn = createInterceptor(this.originalConsole.warn, "WARN");
    console.error = createInterceptor(this.originalConsole.error, "ERROR");
    console.debug = createInterceptor(this.originalConsole.debug, "DEBUG");
  }

  /**
   * Add a log entry directly.
   */
  private addEntry(
    level: LogLevelName,
    source: LogSourceName,
    message: string,
  ): void {
    const entry: LogEntry = {
      id: this.nextId++,
      timestamp: Date.now(),
      level,
      source,
      message,
    };

    this.entries.push(entry);

    // Trim old entries
    if (this.entries.length > this.maxEntries) {
      this.entries = this.entries.slice(-this.maxEntries);
    }

    // Notify subscribers
    for (const sub of this.subscribers) {
      try {
        sub(entry);
      } catch {
        // Ignore subscriber errors
      }
    }
  }

  /**
   * Log a message with the specified level and source.
   */
  log(level: LogLevelName, source: LogSourceName, message: string): void {
    // Also log to console with prefix
    const prefix = `[${source.toUpperCase()}]`;
    const consoleMethod =
      level === "ERROR"
        ? this.originalConsole.error
        : level === "WARN"
          ? this.originalConsole.warn
          : level === "DEBUG"
            ? this.originalConsole.debug
            : this.originalConsole.log;

    consoleMethod(`${prefix} ${message}`);
    this.addEntry(level, source, message);
  }

  debug(source: LogSourceName, message: string): void {
    this.log("DEBUG", source, message);
  }

  info(source: LogSourceName, message: string): void {
    this.log("INFO", source, message);
  }

  warn(source: LogSourceName, message: string): void {
    this.log("WARN", source, message);
  }

  error(source: LogSourceName, message: string): void {
    this.log("ERROR", source, message);
  }

  /**
   * Get all entries.
   */
  getEntries(): LogEntry[] {
    return [...this.entries];
  }

  /**
   * Clear all entries.
   */
  clear(): void {
    this.entries = [];
  }

  /**
   * Subscribe to new log entries.
   */
  subscribe(callback: LogSubscriber): () => void {
    this.subscribers.add(callback);
    return () => this.subscribers.delete(callback);
  }
}

export const logger = new Logger();
