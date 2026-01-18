export type LogLevel = "log" | "error" | "warn" | "info" | "debug" | "result";
export interface LogEntry {
  level: LogLevel;
  message: string;
}

/**
 * Safely stringify a value, handling circular references and BigInt.
 * Falls back to String() if JSON.stringify fails.
 */
function safeStringify(value: unknown): string {
  try {
    return JSON.stringify(value, (_, v) => {
      // Handle BigInt which JSON.stringify can't serialize
      if (typeof v === 'bigint') {
        return v.toString() + 'n';
      }
      return v;
    });
  } catch {
    // Handle circular references or other stringify failures
    try {
      return String(value);
    } catch {
      return '[Unstringifiable value]';
    }
  }
}

export class LogInterceptor {
  private logs: LogEntry[] = [];
  private originalConsole: {
    log: typeof console.log;
    error: typeof console.error;
    warn: typeof console.warn;
    info: typeof console.info;
    debug: typeof console.debug;
  };

  constructor() {
    // Store original console methods
    this.originalConsole = {
      log: console.log,
      error: console.error,
      warn: console.warn,
      info: console.info,
      debug: console.debug,
    };
  }

  /**
   * Start intercepting console logs
   * @param outputToStdout - If true, outputs formatted logs to stdout immediately
   */
  start(): void {
    const createLogger = (level: LogLevel) => (...args: any[]) => {
      const message = args.map(arg =>
        typeof arg === 'string' ? arg :
        arg instanceof Error ? arg.message :
        safeStringify(arg)
      ).join(' ');

      const logEntry: LogEntry = { level, message };
      this.logs.push(logEntry);

      this.originalConsole.log(safeStringify(logEntry));
    };

    console.log = createLogger("log");
    console.error = createLogger("error");
    console.warn = createLogger("warn");
    console.info = createLogger("info");
    console.debug = createLogger("debug");
  }

  /**
   * Add the result as a special log entry
   */
  addResult(message: string): void {
    const logEntry: LogEntry = {
      level: "result",
      message: message,
    };
    this.logs.push(logEntry);
    this.originalConsole.log(safeStringify(logEntry));
  }

  /**
   * Stop intercepting and restore original console methods
   */
  stop(): void {
    Object.assign(console, this.originalConsole);
  }

  /**
   * Get all collected logs
   */
  getLogs(): LogEntry[] {
    return [...this.logs];
  }
}
