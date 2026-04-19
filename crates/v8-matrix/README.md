# v8-matrix

A minimal Rust program that embeds the V8 JavaScript engine and executes JavaScript from Rust.

## What it does

The program demonstrates the core V8 embedding workflow:

1. **Initialize V8** — Creates a default platform and initializes the V8 engine. The platform handles threading and task scheduling for V8 internally.

2. **Create an Isolate** — An `Isolate` is an isolated instance of the V8 VM. Each isolate has its own heap and is completely independent from other isolates. This is the same isolation boundary that Chrome uses to separate tabs.

3. **Set up scopes and context** — V8 requires a `HandleScope` to manage the lifetime of JavaScript objects on the garbage-collected heap, and a `Context` to provide the global object and built-in functions (like `Object`, `Array`, etc.) that JavaScript code expects at runtime.

4. **Compile and run JavaScript** — A JavaScript source string (`"1 + 2 + ' hello from V8!'"`) is compiled into a `Script`, executed, and the result is converted back into a Rust `String` for printing.

## Output

```
JS result: 3 hello from V8!
```

## Dependencies

- [`v8`](https://crates.io/crates/v8) (v130.0.2) — Rust bindings to the V8 JavaScript engine (the same engine that powers Chrome and Node.js). The crate downloads and links a prebuilt V8 binary during compilation.