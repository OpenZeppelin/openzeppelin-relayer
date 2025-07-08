export type LogLevel = "log" | "error" | "warn" | "info" | "debug" | "result";
export interface LogEntry {
  level: LogLevel;
  message: string;
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
        JSON.stringify(arg)
      ).join(' ');

      const logEntry: LogEntry = { level, message };
      this.logs.push(logEntry);

      this.originalConsole.log(JSON.stringify(logEntry));
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
    this.originalConsole.log(JSON.stringify(logEntry));
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
