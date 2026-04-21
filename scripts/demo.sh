#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SERVER_PID=""

cleanup() {
    if [ -n "$SERVER_PID" ]; then
        echo "stopping server (pid $SERVER_PID)..."
        kill "$SERVER_PID" 2>/dev/null || true
        wait "$SERVER_PID" 2>/dev/null || true
    fi
}
trap cleanup EXIT

print_response() {
    python3 -c "
import sys, json
r = json.load(sys.stdin)
if 'error' in r:
    print(f'  ERROR: {r[\"error\"]}')
    sys.exit(1)
m = r['metrics']

print('  stdout:')
for line in r['stdout'].rstrip().split('\n'):
    print(f'    {line}')

if r.get('stderr','').strip():
    print()
    print('  stderr:')
    for line in r['stderr'].rstrip().split('\n'):
        print(f'    {line}')

print()
print('  metrics:')
print(f'    wasm size:     {m[\"wasm_size_bytes\"]:>10,} bytes')
print(f'    engine:        {m[\"engine_us\"]:>10,} us')
print(f'    compile:       {m[\"compile_us\"]:>10,} us')
print(f'    link:          {m[\"link_us\"]:>10,} us')
print(f'    instantiate:   {m[\"instantiate_us\"]:>10,} us')
print(f'    run:           {m[\"run_us\"]:>10,} us')
print(f'    ─────────────────────────')
print(f'    total:         {m[\"total_us\"]:>10,} us  ({m[\"total_us\"]/1000:.1f} ms)')
"
}

echo "=== v8-matrix wasip2 demo ==="
echo

# --- Step 1: Build everything ---
echo "[1/4] Building wasm server..."
cargo build -p v8-matrix-wasm-server --quiet

echo "[2/4] Building hello-wasm (wasm32-wasip2, release)..."
cd "$ROOT/crates/examples/hello-wasm"
cargo build --release --quiet
cd "$ROOT"
HELLO_WASM="$ROOT/crates/examples/hello-wasm/target/wasm32-wasip2/release/hello-wasm.wasm"
echo "  -> $(wc -c < "$HELLO_WASM" | tr -d ' ') bytes"

echo "[3/4] Building wasip2-showcase (wasm32-wasip2, release)..."
cd "$ROOT/crates/examples/wasip2-showcase"
cargo build --release --quiet
cd "$ROOT"
SHOWCASE_WASM="$ROOT/crates/examples/wasip2-showcase/target/wasm32-wasip2/release/wasip2-showcase.wasm"
echo "  -> $(wc -c < "$SHOWCASE_WASM" | tr -d ' ') bytes"

# --- Step 2: Start the server ---
echo "[4/4] Starting server on :3000..."
"$ROOT/target/debug/v8-matrix-wasm-server" &
SERVER_PID=$!

for i in $(seq 1 30); do
    if curl -s http://localhost:3000/run -o /dev/null 2>/dev/null; then
        break
    fi
    sleep 0.1
done
echo "  server ready (pid $SERVER_PID)"
echo

PAYLOAD=$(mktemp /tmp/v8-matrix-demo.XXXX.json)
trap 'rm -f "$PAYLOAD"; cleanup' EXIT

# --- hello-wasm via POST /run ---
echo "╔══════════════════════════════════════╗"
echo "║  POST /run — hello-wasm             ║"
echo "╚══════════════════════════════════════╝"
python3 -c "import json,base64,sys; print(json.dumps({'wasm': base64.b64encode(open(sys.argv[1],'rb').read()).decode(), 'args': ['hello-wasm','foo','bar']}))" "$HELLO_WASM" > "$PAYLOAD"
curl -s -X POST http://localhost:3000/run \
    -H "Content-Type: application/json" \
    -d @"$PAYLOAD" | print_response
echo

# --- wasip2-showcase via GET /demo ---
echo "╔══════════════════════════════════════╗"
echo "║  GET /demo — wasip2-showcase        ║"
echo "╚══════════════════════════════════════╝"
curl -s http://localhost:3000/demo | print_response
echo

echo "=== done ==="
