# v8-matrix

A Rust workspace that runs sandboxed workloads — WASI Preview 2 components and JavaScript fetcher modules — behind an HTTP server. Each invocation spins up a fresh isolate with per-phase execution metrics.

Two execution paths:
- **WASM** — compile Rust to `wasm32-wasip2`, run with WASI capabilities (clocks, random, sockets, filesystem)
- **JS Fetcher** — write a Workers-compatible `fetch(request, env, ctx)` handler, executed in a sandboxed V8 isolate with a full event loop

Includes a browser-based terminal UI where every command you type is a new WASM component invocation.

## Architecture

```
browser (React bundle built with Rspack)
  |
  |  HTTP/JSON (:3000)      — one-shot commands
  |  SSE (:3000)            — streaming fallback
  |  WebTransport (:4433)   — flood via QUIC datagrams (UDP end-to-end)
  v
axum + h3/quinn server (v8-matrix-wasm-server)
  |
  |  WasmConfig { bytes, args, env, network, preopens }
  v
v8-matrix lib
  |
  |-- execute_wasm()            buffered WASM run, returns stdout + stderr + metrics
  |-- execute_wasm_streaming()  channel-based stdout, epoch cancellation
  |-- execute_fetcher()         JS fetcher: V8 isolate + event loop + native fetch
  |-- execute_js()              raw JS evaluation in V8
  v
wasmtime (component model, WASI P2) / V8 (JS fetcher runtime)
```

## Quick Start

```sh
# Prerequisites: Rust, wasm32-wasip2 target
rustup target add wasm32-wasip2

# Build everything and start the server
./web-demo.sh
```

The server prints a Chrome launch command with the SPKI fingerprint for the self-signed cert:
```
open -na 'Google Chrome' --args \
  --origin-to-force-quic-on=127.0.0.1:4433 \
  --ignore-certificate-errors-spki-list=<hash> \
  http://localhost:3000
```

Without Chrome flags, `flood` falls back to SSE automatically.

## Shell Commands

The browser terminal at `http://localhost:3000` supports:

| Command | What it does | WASI interfaces |
|---|---|---|
| `time` | Wall clock timestamp | `wasi:clocks/wall-clock` |
| `env` | List environment variables | `wasi:cli/environment` |
| `rand [n]` | Generate n random bytes (default 16) | `wasi:random` |
| `pi [samples]` | Monte Carlo pi estimation | `wasi:random` + `wasi:clocks` |
| `fib [n]` | Compute fibonacci (default 50) | `wasi:clocks` |
| `sort [n]` | Sort n random integers (default 100k) | `wasi:random` + `wasi:clocks` |
| `bench` | Run all benchmarks | `wasi:clocks` |
| `echo <msg>` | UDP loopback echo | `wasi:sockets/udp` |
| `set <k> <v>` | Store a value | `wasi:filesystem` |
| `get <k>` | Retrieve a value | `wasi:filesystem` |
| `del <k>` | Delete a key | `wasi:filesystem` |
| `keys` | List all stored key/value pairs | `wasi:filesystem` |
| `history` | Show command history | `wasi:filesystem` |
| `flood [bytes]` | Continuous UDP stream (Ctrl+C to stop) | `wasi:sockets/udp` + `wasi:clocks` |
| `help` | List commands | |

State commands (`set`/`get`/`del`/`keys`/`history`) persist across invocations via a preopened directory backed by the host filesystem. Each component instance sees `/state` mapped to a shared temp directory on the server.

## API Endpoints

| Method | Path | Description |
|---|---|---|
| `GET` | `/` | Browser terminal UI |
| `POST` | `/run` | Run arbitrary WASM component (base64 body) |
| `GET` | `/demo` | Run the WASI P2 showcase |
| `GET` | `/exec?cmd=...` | Run a shell command, return JSON |
| `GET` | `/exec/stream?cmd=...` | Run a streaming command, return SSE |

**Response format** (JSON):
```json
{
  "stdout": "...",
  "stderr": "...",
  "metrics": {
    "wasm_size_bytes": 213425,
    "engine_us": 44,
    "compile_us": 131693,
    "link_us": 281,
    "instantiate_us": 242,
    "run_us": 1082,
    "total_us": 133397
  }
}
```

## Workspace Structure

```
crates/
  v8-matrix/                    Core library (V8 + wasmtime + WASI P2)
    src/js_runtime/             JS fetcher runtime
      mod.rs                    Public API: execute_fetcher(), types
      event_loop.rs             Microtask pump + async op polling
      bindings.rs               Native V8 function bindings
      bootstrap/                JS globals loaded before user code
        00_primordials.js       Freeze built-in references
        01_fetch.js             Request/Response/Headers/fetch
        02_encoding.js          TextEncoder/TextDecoder
        03_console.js           console API
  v8-matrix-wasm-server/        Axum HTTP server + static bundle serving
  examples/
    hello-wasm/                 Minimal wasip2 hello world
    wasip2-showcase/            Exercises all WASI P2 interfaces
    wasip2-udp-pingpong/        Interactive shell component (the one behind /exec)
client/                         React app source + Rspack build config
```

### v8-matrix (lib)

The core runtime. Key exports:

- **`execute_wasm(config) -> WasmMetrics`** — Compile + instantiate + run a WASI P2 component synchronously. Captures stdout/stderr via memory pipes. Returns per-phase microsecond timings.
- **`execute_wasm_streaming(config) -> (Receiver, CancelHandle, JoinHandle)`** — Same pipeline but stdout is wired to an `mpsc` channel for real-time streaming. `CancelHandle` uses wasmtime epoch interruption to kill the component on drop (e.g. when the browser disconnects).
- **`execute_fetcher(config) -> FetcherResult`** — Run a JavaScript fetcher module in a sandboxed V8 isolate. Provides a Workers-compatible `(request, env, ctx)` API with a native `fetch()` backed by a Rust event loop. Returns the Response plus execution metrics and captured console output.
- **`execute_js(source) -> String`** — Run raw JavaScript in a V8 isolate.

Configuration via `WasmConfig`:
- `wasm_bytes` — raw `.wasm` component binary
- `args` — CLI arguments (`wasi:cli/args`)
- `env_vars` — environment variables (`wasi:cli/environment`)
- `allow_network` — enable `wasi:sockets` (UDP/TCP)
- `preopens` — host directory mappings (`wasi:filesystem`)

### JS Fetcher Runtime

The fetcher runtime executes JavaScript workloads that implement the Workers-compatible `fetch(request, env, ctx)` pattern. Each invocation gets a fresh V8 isolate with frozen Web API globals, a Rust-driven event loop for async I/O, and configurable environment bindings.

#### Writing a fetcher module

```js
module.exports = {
  async fetch(request, env, ctx) {
    // globalThis.fetch is provided by the runtime — frozen, non-writable
    const resp = await fetch(env.API_BASE + "/users");
    const data = await resp.json();

    // Full Request/Response/Headers API available
    console.log("fetched %d users", data.length);

    return new Response(JSON.stringify({ users: data }), {
      status: 200,
      headers: { "content-type": "application/json" },
    });
  }
};
```

**Parameters:**
- `request` — standard `Request` object (url, method, headers, body)
- `env` — frozen object of dynamic bindings (`env.API_KEY`, `env.CONFIG`, etc.)
- `ctx` — execution context (`ctx.waitUntil(promise)`, `ctx.passThroughOnException()`)

**Globals provided (all frozen on `globalThis`):**
- `fetch()` — backed by native HTTP via reqwest
- `Request`, `Response`, `Headers`
- `TextEncoder`, `TextDecoder`
- `console.log/warn/error/info/debug/trace/time/timeEnd`
- `setTimeout`, `clearTimeout`

#### Calling from Rust

```rust
use v8_matrix::{execute_fetcher, FetcherConfig, JsRequest, BindingValue, ResourceLimits};
use std::collections::HashMap;

let config = FetcherConfig {
    script: r#"
        module.exports = {
            async fetch(request, env, ctx) {
                const resp = await fetch(env.API_BASE + "/health");
                const body = await resp.text();
                return new Response(body, {
                    status: resp.status,
                    headers: { "x-upstream-status": String(resp.status) },
                });
            }
        };
    "#.to_string(),
    request: JsRequest {
        url: "https://example.com/incoming".into(),
        method: "GET".into(),
        headers: vec![("authorization".into(), "Bearer tok_123".into())],
        body: None,
    },
    bindings: HashMap::from([
        ("API_BASE".into(), BindingValue::Text("https://api.example.com".into())),
        ("CONFIG".into(), BindingValue::Json(serde_json::json!({
            "retries": 3,
            "timeout_ms": 5000
        }))),
    ]),
    limits: ResourceLimits::default(),
};

let result = execute_fetcher(&config).unwrap();

println!("status: {}", result.response.status);           // 200
println!("body: {}", result.response.body);                // upstream response
println!("setup: {}us", result.metrics.setup_us);          // V8 isolate + bootstrap
println!("compile: {}us", result.metrics.compile_us);      // user script compilation
println!("execute: {}us", result.metrics.execute_us);      // event loop + fetch handler
println!("total: {}us", result.metrics.total_us);          // wall clock

// Console output captured as structured data
for (level, msg) in &result.metrics.console_output {
    println!("[{level}] {msg}");
}
```

#### Resource limits

```rust
ResourceLimits {
    max_duration_ms: 30_000,           // 30s wall clock timeout
    max_heap_bytes: 256 * 1024 * 1024, // 256MB V8 heap
    max_event_loop_iterations: 10_000, // prevent infinite async loops
}
```

#### Execution flow

```
fetch(request, env, ctx) called
        |
        v
  returns Promise
        |
        v
  Event Loop (Rust):
    1. drain V8 microtask queue
    2. promise settled? -> extract Response, done
    3. execute pending fetch ops on tokio (reqwest)
    4. resolve/reject V8 promises with results
    5. fire ready setTimeout callbacks
    6. goto 1
```

### Example Components

All compile to `wasm32-wasip2` with `opt-level = "z"`, LTO, and strip for minimal binary size.

Build individually:
```sh
cd crates/examples/hello-wasm && cargo build --release     # ~77 KB
cd crates/examples/wasip2-showcase && cargo build --release # ~147 KB
cd crates/examples/wasip2-udp-pingpong && cargo build --release # ~213 KB
```

## CLI Demo

```sh
./demo.sh
```

Builds everything, starts the server, sends requests via curl, and prints formatted output with metrics:

```
=== v8-matrix wasip2 demo ===

[1/4] Building wasm server...
[2/4] Building hello-wasm (wasm32-wasip2, release)...
  -> 76852 bytes
...

--- POST /run (hello-wasm) ---
  stdout:
    Hello from WebAssembly (WASI P2)!
    Sum of 1..10 = 55

  metrics:
    compile:        70,222 us
    run:               654 us
    total:          73,790 us  (73.8 ms)
```

## Dependencies

- [wasmtime](https://wasmtime.dev/) 29 — WASI P2 component model runtime
- [rusty_v8](https://crates.io/crates/v8) 130 — V8 JavaScript engine bindings (JS fetcher runtime)
- [reqwest](https://crates.io/crates/reqwest) 0.12 — HTTP client backing `globalThis.fetch`
- [axum](https://github.com/tokio-rs/axum) 0.8 — HTTP server
- [tokio](https://tokio.rs/) — async runtime + event loop driver
