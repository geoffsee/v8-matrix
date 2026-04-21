use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use h3::ext::Protocol;
use h3_webtransport::server::WebTransportSession;
use http::Method;
use quinn::crypto::rustls::QuicServerConfig;
use v8_matrix::{Preopen, WasmConfig, execute_wasm_streaming};

use crate::AppState;

fn load_wasm() -> Vec<u8> {
    let path = std::env::var("UDP_WASM_PATH").unwrap_or_else(|_| {
        "crates/examples/wasip2-udp-pingpong/target/wasm32-wasip2/release/wasip2-udp-pingpong.wasm"
            .to_string()
    });
    std::fs::read(&path).unwrap_or_else(|e| panic!("failed to read {path}: {e}"))
}

pub async fn run(
    tls_config: Arc<rustls::ServerConfig>,
    bind_addr: std::net::SocketAddr,
    state: Arc<AppState>,
) -> anyhow::Result<()> {
    let mut server_config =
        quinn::ServerConfig::with_crypto(Arc::new(QuicServerConfig::try_from(
            tls_config.as_ref().clone(),
        )?));

    let mut transport = quinn::TransportConfig::default();
    transport.keep_alive_interval(Some(Duration::from_secs(2)));
    transport.max_idle_timeout(Some(Duration::from_secs(30).try_into()?));
    server_config.transport_config(Arc::new(transport));

    let endpoint = quinn::Endpoint::server(server_config, bind_addr)?;
    println!("webtransport listening on https://{bind_addr}");

    while let Some(incoming) = endpoint.accept().await {
        let state = state.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_connection(incoming, state).await {
                eprintln!("webtransport connection error: {e}");
            }
        });
    }

    Ok(())
}

async fn handle_connection(
    incoming: quinn::Incoming,
    state: Arc<AppState>,
) -> anyhow::Result<()> {
    let conn = incoming.await?;

    let mut h3_conn = h3::server::builder()
        .enable_webtransport(true)
        .enable_extended_connect(true)
        .enable_datagram(true)
        .max_webtransport_sessions(1)
        .build(h3_quinn::Connection::new(conn))
        .await?;

    loop {
        match h3_conn.accept().await {
            Ok(Some(resolver)) => {
                let (req, stream) = resolver.resolve_request().await?;
                let ext = req.extensions();
                if req.method() == Method::CONNECT
                    && ext.get::<Protocol>() == Some(&Protocol::WEB_TRANSPORT)
                {
                    let session =
                        WebTransportSession::accept(req, stream, h3_conn).await?;
                    handle_session(session, state).await;
                    return Ok(());
                }
            }
            Ok(None) => return Ok(()),
            Err(e) => return Err(e.into()),
        }
    }
}

async fn handle_session(
    session: WebTransportSession<h3_quinn::Connection, Bytes>,
    state: Arc<AppState>,
) {
    let wasm_bytes = load_wasm();
    let state_dir = state.state_dir.clone();

    let result = execute_wasm_streaming(&WasmConfig {
        wasm_bytes: &wasm_bytes,
        args: &["wasi-shell".into(), "flood".into(), "64".into()],
        env_vars: &[],
        allow_network: true,
        preopens: &[Preopen {
            host_path: state_dir,
            guest_path: "/state".into(),
        }],
    });

    let (rx, cancel, _handle) = match result {
        Ok(v) => v,
        Err(e) => {
            eprintln!("webtransport: wasm error: {e}");
            return;
        }
    };

    // cancel guard — kills the component when we exit this function
    let _cancel = cancel;

    let mut sender = session.datagram_sender();

    loop {
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(data) => {
                if sender.send_datagram(Bytes::from(data)).is_err() {
                    break; // client disconnected
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                tokio::task::yield_now().await;
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
}
