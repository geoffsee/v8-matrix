# v8-matrix

A Rust workspace that runs WASI Preview 2 components behind an HTTP server. Each request spins up a fresh sandboxed WebAssembly component with access to WASI capabilities (clocks, random, sockets, filesystem) and returns structured output with per-phase execution metrics.

Includes a browser-based terminal UI where every command you type is a new WASM component invocation.

## Architecture

```
browser (demo.html)
  |
  |  HTTP / SSE
  v
axum server (v8-matrix-wasm-server)
  |
  |  WasmConfig { bytes, args, env, network, preopens }
  v
v8-matrix lib
  |
  |-- execute_wasm()            buffered run, returns stdout + stderr + metrics
  |-- execute_wasm_streaming()  SSE via channel-based stdout, epoch cancellation
  |-- execute_js()              V8 isolate for raw JavaScript
  v
wasmtime (component model, WASI P2)
```

## Quick Start

```sh
# Prerequisites: Rust nightly, wasm32-wasip2 target
rustup target add wasm32-wasip2

# Build everything and start the server
./web-demo.sh

# Open http://localhost:3000
```

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
  v8-matrix-wasm-server/        Axum HTTP server + demo.html
  examples/
    hello-wasm/                 Minimal wasip2 hello world
    wasip2-showcase/            Exercises all WASI P2 interfaces
    wasip2-udp-pingpong/        Interactive shell component (the one behind /exec)
```

### v8-matrix (lib)

The core runtime. Key exports:

- **`execute_wasm(config) -> WasmMetrics`** — Compile + instantiate + run a WASI P2 component synchronously. Captures stdout/stderr via memory pipes. Returns per-phase microsecond timings.
- **`execute_wasm_streaming(config) -> (Receiver, CancelHandle, JoinHandle)`** — Same pipeline but stdout is wired to an `mpsc` channel for real-time streaming. `CancelHandle` uses wasmtime epoch interruption to kill the component on drop (e.g. when the browser disconnects).
- **`execute_js(source) -> String`** — Run JavaScript in a V8 isolate.

Configuration via `WasmConfig`:
- `wasm_bytes` — raw `.wasm` component binary
- `args` — CLI arguments (`wasi:cli/args`)
- `env_vars` — environment variables (`wasi:cli/environment`)
- `allow_network` — enable `wasi:sockets` (UDP/TCP)
- `preopens` — host directory mappings (`wasi:filesystem`)

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
- [rusty_v8](https://crates.io/crates/v8) 130 — V8 JavaScript engine bindings
- [axum](https://github.com/tokio-rs/axum) 0.8 — HTTP server
- [tokio](https://tokio.rs/) — async runtime
