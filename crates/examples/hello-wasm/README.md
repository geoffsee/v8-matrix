# hello-wasm

A minimal Rust program that compiles to `wasm32-wasip2` (WASI Preview 2).

## Building

The crate's `.cargo/config.toml` sets the default target to `wasm32-wasip2`, so a plain `cargo build` produces a `.wasm` binary:

```sh
# Install the target (one-time)
rustup target add wasm32-wasip2

# Build
cd crates/examples/hello-wasm
cargo build
```

Output: `target/wasm32-wasip2/debug/hello-wasm.wasm`

## Running

Use any WASI-compatible runtime (e.g. Wasmtime):

```sh
wasmtime target/wasm32-wasip2/debug/hello-wasm.wasm
```
