use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::Arc;

use axum::{
    Router,
    extract::{Json, Query, State},
    extract::DefaultBodyLimit,
    http::StatusCode,
    response::{Html, Sse, sse::Event},
    routing::{get, post},
};
use futures::stream::Stream;
use serde::{Deserialize, Serialize};
use v8_matrix::{Preopen, WasmConfig, execute_wasm, execute_wasm_streaming};

struct AppState {
    state_dir: PathBuf,
}

#[derive(Deserialize)]
struct WasmRequest {
    wasm: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: Vec<(String, String)>,
    #[serde(default)]
    allow_network: bool,
}

#[derive(Serialize)]
struct WasmResponse {
    stdout: String,
    stderr: String,
    metrics: Metrics,
}

#[derive(Serialize)]
struct Metrics {
    wasm_size_bytes: usize,
    engine_us: u128,
    compile_us: u128,
    link_us: u128,
    instantiate_us: u128,
    run_us: u128,
    total_us: u128,
}

#[derive(Serialize)]
struct ErrorResponse {
    error: String,
}

fn metrics_response(m: v8_matrix::WasmMetrics) -> WasmResponse {
    WasmResponse {
        stdout: m.stdout,
        stderr: m.stderr,
        metrics: Metrics {
            wasm_size_bytes: m.wasm_size,
            engine_us: m.engine_us,
            compile_us: m.compile_us,
            link_us: m.link_us,
            instantiate_us: m.instantiate_us,
            run_us: m.run_us,
            total_us: m.total_us,
        },
    }
}

fn err(status: StatusCode, msg: String) -> (StatusCode, Json<ErrorResponse>) {
    (status, Json(ErrorResponse { error: msg }))
}

fn load_wasm(env_key: &str, default: &str) -> Result<Vec<u8>, (StatusCode, Json<ErrorResponse>)> {
    let path = std::env::var(env_key).unwrap_or_else(|_| default.to_string());
    std::fs::read(&path).map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, format!("{path}: {e}")))
}

async fn run_wasm_handler(
    Json(req): Json<WasmRequest>,
) -> Result<Json<WasmResponse>, (StatusCode, Json<ErrorResponse>)> {
    use base64::Engine;
    let wasm_bytes = base64::engine::general_purpose::STANDARD
        .decode(&req.wasm)
        .map_err(|e| err(StatusCode::BAD_REQUEST, format!("invalid base64: {e}")))?;

    let m = tokio::task::spawn_blocking(move || {
        execute_wasm(&WasmConfig {
            wasm_bytes: &wasm_bytes,
            args: &req.args,
            env_vars: &req.env,
            allow_network: req.allow_network,
            preopens: &[],
        })
    })
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, format!("task failed: {e}")))?
    .map_err(|e| err(StatusCode::BAD_REQUEST, e))?;

    Ok(Json(metrics_response(m)))
}

async fn demo_showcase() -> Result<Json<WasmResponse>, (StatusCode, Json<ErrorResponse>)> {
    let wasm_bytes = load_wasm(
        "DEMO_WASM_PATH",
        "crates/examples/wasip2-showcase/target/wasm32-wasip2/release/wasip2-showcase.wasm",
    )?;

    let m = tokio::task::spawn_blocking(move || {
        execute_wasm(&WasmConfig {
            wasm_bytes: &wasm_bytes,
            args: &["wasip2-showcase".into(), "--demo".into()],
            env_vars: &[
                ("RUNTIME".into(), "v8-matrix".into()),
                ("WASI_VERSION".into(), "preview2".into()),
                ("DEMO".into(), "true".into()),
            ],
            allow_network: false,
            preopens: &[],
        })
    })
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, format!("task failed: {e}")))?
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(Json(metrics_response(m)))
}

#[derive(Deserialize)]
struct CmdQuery {
    #[serde(default)]
    cmd: String,
}

async fn exec_cmd(
    State(state): State<Arc<AppState>>,
    Query(q): Query<CmdQuery>,
) -> Result<Json<WasmResponse>, (StatusCode, Json<ErrorResponse>)> {
    let wasm_bytes = load_wasm(
        "UDP_WASM_PATH",
        "crates/examples/wasip2-udp-pingpong/target/wasm32-wasip2/release/wasip2-udp-pingpong.wasm",
    )?;

    let parts: Vec<String> = q.cmd.split_whitespace().map(String::from).collect();
    let needs_network = parts.first().map(|s| s.as_str()) == Some("echo");

    let mut args = vec!["wasi-shell".to_string()];
    args.extend(parts);

    let state_dir = state.state_dir.clone();
    let m = tokio::task::spawn_blocking(move || {
        execute_wasm(&WasmConfig {
            wasm_bytes: &wasm_bytes,
            args: &args,
            env_vars: &[
                ("RUNTIME".into(), "v8-matrix".into()),
                ("WASI_VERSION".into(), "preview2".into()),
            ],
            allow_network: needs_network,
            preopens: &[Preopen {
                host_path: state_dir,
                guest_path: "/state".into(),
            }],
        })
    })
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, format!("task failed: {e}")))?
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(Json(metrics_response(m)))
}

async fn exec_stream(
    State(state): State<Arc<AppState>>,
    Query(q): Query<CmdQuery>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let wasm_bytes = load_wasm(
        "UDP_WASM_PATH",
        "crates/examples/wasip2-udp-pingpong/target/wasm32-wasip2/release/wasip2-udp-pingpong.wasm",
    )
    .unwrap_or_default();

    let parts: Vec<String> = q.cmd.split_whitespace().map(String::from).collect();
    let mut args = vec!["wasi-shell".to_string()];
    args.extend(parts);

    let state_dir = state.state_dir.clone();
    let (rx, cancel, _handle) = execute_wasm_streaming(&WasmConfig {
        wasm_bytes: &wasm_bytes,
        args: &args,
        env_vars: &[],
        allow_network: true,
        preopens: &[Preopen {
            host_path: state_dir,
            guest_path: "/state".into(),
        }],
    })
    .unwrap();

    let stream = async_stream::stream! {
        // Guard: cancel the component when this stream is dropped (client disconnect)
        let _cancel_guard = cancel;

        loop {
            match rx.recv_timeout(std::time::Duration::from_millis(50)) {
                Ok(data) => {
                    let text = String::from_utf8_lossy(&data).to_string();
                    for line in text.lines() {
                        yield Ok::<_, Infallible>(Event::default().data(line.to_string()));
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    tokio::task::yield_now().await;
                    continue;
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        // _cancel_guard dropped here or on stream Drop
    };

    Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::new().interval(std::time::Duration::from_secs(1)),
    )
}

async fn index() -> Html<&'static str> {
    Html(include_str!("../static/demo.html"))
}

#[tokio::main]
async fn main() {
    v8_matrix::init_v8();

    let state_dir = std::env::temp_dir().join("v8-matrix-state");
    std::fs::create_dir_all(&state_dir).expect("create state dir");
    println!("state dir: {}", state_dir.display());

    let shared = Arc::new(AppState { state_dir });

    let app = Router::new()
        .route("/", get(index))
        .route("/run", post(run_wasm_handler))
        .route("/demo", get(demo_showcase))
        .route("/exec", get(exec_cmd))
        .route("/exec/stream", get(exec_stream))
        .layer(DefaultBodyLimit::max(50 * 1024 * 1024))
        .with_state(shared);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    println!("listening on http://0.0.0.0:3000");
    axum::serve(listener, app).await.unwrap();
}
