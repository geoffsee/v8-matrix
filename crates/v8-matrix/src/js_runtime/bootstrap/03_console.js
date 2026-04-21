// 03_console.js — console.log/warn/error/info/debug/trace/time/timeEnd
// Backed by __native_console_log(level, message) injected from Rust.

((globalThis) => {
  "use strict";

  const { ObjectDefineProperty, ObjectFreeze, JSONStringify } =
    globalThis.__primordials;

  function formatValue(v) {
    if (v === undefined) return "undefined";
    if (v === null) return "null";
    if (typeof v === "string") return v;
    if (typeof v === "number" || typeof v === "boolean" || typeof v === "bigint") {
      return String(v);
    }
    if (typeof v === "symbol") return v.toString();
    if (typeof v === "function") return `[Function: ${v.name || "anonymous"}]`;
    if (v instanceof Error) {
      return v.stack || `${v.name}: ${v.message}`;
    }
    try {
      return JSONStringify(v, null, 2);
    } catch {
      return String(v);
    }
  }

  function formatArgs(args) {
    if (args.length === 0) return "";

    // Simple printf-style %s, %d, %o, %j support
    const first = args[0];
    if (typeof first === "string" && args.length > 1) {
      let i = 1;
      const formatted = first.replace(/%[sdoOj%]/g, (match) => {
        if (match === "%%") return "%";
        if (i >= args.length) return match;
        const arg = args[i++];
        switch (match) {
          case "%s":
            return String(arg);
          case "%d":
            return Number(arg).toString();
          case "%o":
          case "%O":
          case "%j":
            return formatValue(arg);
          default:
            return match;
        }
      });
      const remaining = args.slice(i).map(formatValue);
      return remaining.length > 0
        ? formatted + " " + remaining.join(" ")
        : formatted;
    }

    return args.map(formatValue).join(" ");
  }

  const timers = new Map();

  const console = {
    log(...args) {
      __native_console_log("log", formatArgs(args));
    },
    info(...args) {
      __native_console_log("info", formatArgs(args));
    },
    warn(...args) {
      __native_console_log("warn", formatArgs(args));
    },
    error(...args) {
      __native_console_log("error", formatArgs(args));
    },
    debug(...args) {
      __native_console_log("debug", formatArgs(args));
    },
    trace(...args) {
      const err = new Error();
      const stack = err.stack
        ? err.stack.split("\n").slice(2).join("\n")
        : "";
      __native_console_log(
        "trace",
        (args.length > 0 ? formatArgs(args) + "\n" : "Trace\n") + stack,
      );
    },
    assert(condition, ...args) {
      if (!condition) {
        const msg =
          args.length > 0
            ? "Assertion failed: " + formatArgs(args)
            : "Assertion failed";
        __native_console_log("error", msg);
      }
    },
    time(label = "default") {
      timers.set(String(label), Date.now());
    },
    timeEnd(label = "default") {
      const key = String(label);
      const start = timers.get(key);
      if (start === undefined) {
        __native_console_log("warn", `Timer '${key}' does not exist`);
        return;
      }
      timers.delete(key);
      __native_console_log("log", `${key}: ${Date.now() - start}ms`);
    },
    timeLog(label = "default", ...args) {
      const key = String(label);
      const start = timers.get(key);
      if (start === undefined) {
        __native_console_log("warn", `Timer '${key}' does not exist`);
        return;
      }
      const elapsed = `${key}: ${Date.now() - start}ms`;
      const extra = args.length > 0 ? " " + formatArgs(args) : "";
      __native_console_log("log", elapsed + extra);
    },
    count: (() => {
      const counters = new Map();
      return (label = "default") => {
        const key = String(label);
        const c = (counters.get(key) || 0) + 1;
        counters.set(key, c);
        __native_console_log("log", `${key}: ${c}`);
      };
    })(),
    countReset: (() => {
      const counters = new Map();
      return (label = "default") => {
        counters.delete(String(label));
      };
    })(),
    dir(obj) {
      __native_console_log("log", formatValue(obj));
    },
    clear() {
      // no-op in sandboxed environment
    },
  };

  ObjectFreeze(console);

  ObjectDefineProperty(globalThis, "console", {
    value: console,
    writable: false,
    enumerable: false,
    configurable: false,
  });
})(globalThis);
