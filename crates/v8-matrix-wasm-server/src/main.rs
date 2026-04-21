mod cert;
mod webtransport;

use std::convert::Infallible;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::{
    Router,
    extract::{Json, Query, State},
    extract::DefaultBodyLimit,
    http::StatusCode,
    response::{Sse, sse::Event},
    routing::{get, post},
};
use futures::stream::Stream;
use jsonwebtoken::{EncodingKey, Header, encode};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tower_http::services::ServeDir;
use v8_matrix::{Preopen, WasmConfig, execute_wasm, execute_wasm_streaming};

pub struct AppState {
    pub state_dir: PathBuf,
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
    #[serde(default)]
    ctx: Option<Value>,
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

#[derive(Deserialize)]
struct GenerateJwtRequest {
    username: String,
    org: String,
    #[serde(default)]
    roles: Vec<String>,
    #[serde(flatten)]
    custom: HashMap<String, Value>,
}

#[derive(Serialize)]
struct JwtResponse {
    token: String,
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

fn tenant_id_from_org(org: &str) -> String {
    let slug = org
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_string();

    if slug.is_empty() {
        "tenant".to_string()
    } else {
        slug
    }
}

async fn run_wasm_handler(
    Json(mut req): Json<WasmRequest>,
) -> Result<Json<WasmResponse>, (StatusCode, Json<ErrorResponse>)> {
    use base64::Engine;
    let wasm_bytes = base64::engine::general_purpose::STANDARD
        .decode(&req.wasm)
        .map_err(|e| err(StatusCode::BAD_REQUEST, format!("invalid base64: {e}")))?;

    let mut env_vars = req.env.clone();
    if let Some(ctx) = req.ctx.take() {
        if let Ok(json_str) = serde_json::to_string(&ctx) {
            env_vars.push(("CF_CTX_JSON".to_string(), json_str));
        }
    }

    let m = tokio::task::spawn_blocking(move || {
        execute_wasm(&WasmConfig {
            wasm_bytes: &wasm_bytes,
            args: &req.args,
            env_vars: &env_vars,
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

async fn generate_jwt(
    Json(req): Json<GenerateJwtRequest>,
) -> Result<Json<JwtResponse>, (StatusCode, Json<ErrorResponse>)> {
    let secret = std::env::var("JWT_SECRET").unwrap_or_else(|_| {
        eprintln!(
            "WARNING: JWT_SECRET not set - using insecure default (change in production!)"
        );
        "CHANGE_ME_IN_PRODUCTION_32_BYTES_MINIMUM_abcdefghijklmnopqrstuvwxyz".to_string()
    });

    if secret.len() < 32 {
        eprintln!("WARNING: JWT_SECRET is too short for security");
    }

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as usize;

    let mut claims = serde_json::Map::new();
    claims.insert("sub".into(), json!(req.username));
    claims.insert("org".into(), json!(req.org));
    claims.insert("tenant_id".into(), json!(tenant_id_from_org(&req.org)));
    if !req.roles.is_empty() {
        claims.insert("roles".into(), json!(req.roles));
    }
    claims.insert("iat".into(), json!(now));
    claims.insert("exp".into(), json!(now + 86_400));

    for (k, v) in req.custom {
        claims.insert(k, v);
    }

    let token = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, format!("JWT encode failed: {e}")))?;

    Ok(Json(JwtResponse { token }))
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

#[tokio::main]
async fn main() {
    rustls::crypto::ring::default_provider()
        .install_default()
        .ok();

    v8_matrix::init_v8();

    let state_dir = std::env::temp_dir().join("v8-matrix-state");
    std::fs::create_dir_all(&state_dir).expect("create state dir");

    let shared = Arc::new(AppState { state_dir });

    let acme_domain = std::env::var("ACME_DOMAIN").ok();

    let tls = if let Some(ref domain) = acme_domain {
        println!("acme: provisioning cert for {domain}");
        cert::acme(domain)
    } else {
        cert::generate_self_signed()
    };

    let cert_hash = match &tls {
        cert::TlsMode::SelfSigned { spki_hash_b64, .. } => spki_hash_b64.clone(),
        cert::TlsMode::Acme { .. } => String::new(),
    };

    // WebTransport (QUIC/UDP) — shares ACME resolver in production
    let quinn_tls = tls.quinn_rustls_config();
    let wt_bind: std::net::SocketAddr = if acme_domain.is_some() {
        "0.0.0.0:443".parse().unwrap()
    } else {
        "0.0.0.0:4433".parse().unwrap()
    };
    let wt_state = shared.clone();
    tokio::spawn(async move {
        if let Err(e) = webtransport::run(quinn_tls, wt_bind, wt_state).await {
            eprintln!("webtransport server error: {e}");
        }
    });

    let app = Router::new()
        .route("/run", post(run_wasm_handler))
        .route("/jwt", post(generate_jwt))
        .route("/demo", get(demo_showcase))
        .route("/exec", get(exec_cmd))
        .route("/exec/stream", get(exec_stream))
        .route("/cert-hash", get(move || {
            let hash = cert_hash.clone();
            async move { hash }
        }))
        .layer(DefaultBodyLimit::max(50 * 1024 * 1024))
        .fallback_service(
            ServeDir::new("crates/v8-matrix-wasm-server/static")
                .append_index_html_on_directories(true),
        )
        .with_state(shared);

    // Drive ACME renewal in background, print Chrome flags for self-signed
    match tls {
        cert::TlsMode::Acme { acme_state } => {
            use futures::StreamExt;
            let mut state = acme_state;
            tokio::spawn(async move {
                loop {
                    match state.next().await {
                        Some(Ok(ok)) => println!("acme: {ok:?}"),
                        Some(Err(err)) => eprintln!("acme error: {err:?}"),
                        None => break,
                    }
                }
            });
        }
        cert::TlsMode::SelfSigned { ref spki_hash_b64, .. } => {
            println!();
            println!("Launch Chrome with:");
            println!(
                "  open -na 'Google Chrome' --args \
                 --origin-to-force-quic-on=127.0.0.1:4433 \
                 --ignore-certificate-errors-spki-list={spki_hash_b64} \
                 http://localhost:<PORT>",
            );
            println!();
        }
    }

    // Always plain HTTP on an ephemeral port — Fly handles TLS in production
    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    println!("http on http://{addr}");
    println!("open http://127.0.0.1:{} in your browser", addr.port());
    axum::serve(listener, app).await.unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{Body, to_bytes};
    use axum::http::Request;
    use tower::util::ServiceExt;

    #[tokio::test]
    async fn test_generate_jwt() {
        let app = Router::new().route("/jwt", post(generate_jwt));

        let payload = r#"{
            "username": "testuser",
            "org": "testorg",
            "roles": ["user"]
        }"#;

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/jwt")
                    .header("content-type", "application/json")
                    .body(Body::from(payload))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json["token"].is_string() && !json["token"].as_str().unwrap().is_empty());

        let token = json["token"].as_str().unwrap();
        let claims_b64 = token.split('.').nth(1).unwrap();
        use base64::Engine;
        let claims_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(claims_b64)
            .unwrap();
        let claims: serde_json::Value = serde_json::from_slice(&claims_bytes).unwrap();
        assert_eq!(claims["tenant_id"], "testorg");
    }
}
