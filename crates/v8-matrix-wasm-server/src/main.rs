use axum::{Router, extract::Json, extract::DefaultBodyLimit, http::StatusCode, routing::post};
use serde::{Deserialize, Serialize};
use v8_matrix::execute_wasm;

#[derive(Deserialize)]
struct WasmRequest {
    /// Base64-encoded .wasm component binary
    wasm: String,
    /// CLI arguments to pass to the component
    #[serde(default)]
    args: Vec<String>,
}

#[derive(Serialize)]
struct WasmResponse {
    stdout: String,
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

async fn run_wasm(
    Json(req): Json<WasmRequest>,
) -> Result<Json<WasmResponse>, (StatusCode, Json<ErrorResponse>)> {
    use base64::Engine;
    let wasm_bytes = base64::engine::general_purpose::STANDARD
        .decode(&req.wasm)
        .map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse { error: format!("invalid base64: {e}") }),
            )
        })?;

    let m = tokio::task::spawn_blocking(move || {
        execute_wasm(&wasm_bytes, &req.args)
    })
    .await
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse { error: format!("task failed: {e}") }),
        )
    })?
    .map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse { error: e }),
        )
    })?;

    Ok(Json(WasmResponse {
        stdout: m.stdout,
        metrics: Metrics {
            wasm_size_bytes: m.wasm_size,
            engine_us: m.engine_us,
            compile_us: m.compile_us,
            link_us: m.link_us,
            instantiate_us: m.instantiate_us,
            run_us: m.run_us,
            total_us: m.total_us,
        },
    }))
}

#[tokio::main]
async fn main() {
    v8_matrix::init_v8();

    let app = Router::new()
        .route("/run", post(run_wasm))
        .layer(DefaultBodyLimit::max(50 * 1024 * 1024));

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    println!("listening on http://0.0.0.0:3000");
    axum::serve(listener, app).await.unwrap();
}
