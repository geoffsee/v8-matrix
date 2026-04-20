use std::sync::Once;

use bytes::Bytes;
use wasmtime_wasi::{HostOutputStream, StdoutStream, StreamResult};

static V8_INIT: Once = Once::new();

/// Initialize V8 (safe to call multiple times).
pub fn init_v8() {
    V8_INIT.call_once(|| {
        let platform = v8::new_default_platform(0, false).make_shared();
        v8::V8::initialize_platform(platform);
        v8::V8::initialize();
    });
}

/// Execute a JavaScript source string and return the result as a string.
pub fn execute_js(source: &str) -> Result<String, String> {
    init_v8();

    let isolate = &mut v8::Isolate::new(v8::CreateParams::default());
    let handle_scope = &mut v8::HandleScope::new(isolate);
    let context = v8::Context::new(handle_scope, Default::default());
    let scope = &mut v8::ContextScope::new(handle_scope, context);

    let code = v8::String::new(scope, source).ok_or("failed to create source string")?;
    let script = v8::Script::compile(scope, code, None).ok_or("failed to compile script")?;
    let result = script.run(scope).ok_or("script execution failed")?;
    let result_str = result.to_string(scope).ok_or("failed to convert result to string")?;

    Ok(result_str.to_rust_string_lossy(scope))
}

/// Metrics from a WASM execution run.
pub struct WasmMetrics {
    /// Captured stdout from the component.
    pub stdout: String,
    /// Captured stderr from the component.
    pub stderr: String,
    /// Size of the input wasm component in bytes.
    pub wasm_size: usize,
    /// Time to create the wasmtime engine.
    pub engine_us: u128,
    /// Time to compile the component.
    pub compile_us: u128,
    /// Time to link WASI imports.
    pub link_us: u128,
    /// Time to instantiate the component.
    pub instantiate_us: u128,
    /// Time to execute the component's run function.
    pub run_us: u128,
    /// Total wall-clock time.
    pub total_us: u128,
}

/// A preopened directory mapping.
pub struct Preopen {
    pub host_path: std::path::PathBuf,
    pub guest_path: String,
}

/// Configuration for executing a WASI P2 component.
pub struct WasmConfig<'a> {
    pub wasm_bytes: &'a [u8],
    pub args: &'a [String],
    pub env_vars: &'a [(String, String)],
    pub allow_network: bool,
    pub preopens: &'a [Preopen],
}

/// Execute a WASI P2 component module.
///
/// Runs the component's `wasi:cli/run` entry point, captures stdout and stderr,
/// and returns metrics for each phase.
pub fn execute_wasm(config: &WasmConfig) -> Result<WasmMetrics, String> {
    use std::time::Instant;
    use wasmtime::component::{Component, Linker, ResourceTable};
    use wasmtime::{Config, Engine, Store};
    use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiView, bindings::sync::CommandPre, pipe::MemoryOutputPipe};

    struct State {
        ctx: WasiCtx,
        table: ResourceTable,
    }

    impl WasiView for State {
        fn ctx(&mut self) -> &mut WasiCtx {
            &mut self.ctx
        }
        fn table(&mut self) -> &mut ResourceTable {
            &mut self.table
        }
    }

    let total_start = Instant::now();
    let wasm_size = config.wasm_bytes.len();

    // Engine creation
    let t = Instant::now();
    let mut eng_config = Config::new();
    eng_config.wasm_component_model(true);
    let engine = Engine::new(&eng_config).map_err(|e| e.to_string())?;
    let engine_us = t.elapsed().as_micros();

    // Component compilation
    let t = Instant::now();
    let component = Component::new(&engine, config.wasm_bytes).map_err(|e| e.to_string())?;
    let compile_us = t.elapsed().as_micros();

    // Linking
    let t = Instant::now();
    let mut linker = Linker::<State>::new(&engine);
    wasmtime_wasi::add_to_linker_sync(&mut linker).map_err(|e| e.to_string())?;
    let link_us = t.elapsed().as_micros();

    // Store + WASI context setup
    let stdout_pipe = MemoryOutputPipe::new(64 * 1024);
    let stderr_pipe = MemoryOutputPipe::new(64 * 1024);
    let mut builder = WasiCtxBuilder::new();
    builder.stdout(stdout_pipe.clone());
    builder.stderr(stderr_pipe.clone());
    builder.args(config.args);
    builder.envs(config.env_vars);
    if config.allow_network {
        builder.inherit_network();
        builder.allow_udp(true);
        builder.allow_tcp(true);
    }
    for preopen in config.preopens {
        builder.preopened_dir(
            &preopen.host_path,
            &preopen.guest_path,
            wasmtime_wasi::DirPerms::all(),
            wasmtime_wasi::FilePerms::all(),
        ).map_err(|e| e.to_string())?;
    }
    let state = State {
        ctx: builder.build(),
        table: ResourceTable::new(),
    };
    let mut store = Store::new(&engine, state);

    // Instantiation
    let t = Instant::now();
    let pre = CommandPre::new(
        linker.instantiate_pre(&component).map_err(|e| e.to_string())?
    ).map_err(|e| e.to_string())?;
    let command = pre.instantiate(&mut store).map_err(|e| e.to_string())?;
    let instantiate_us = t.elapsed().as_micros();

    // Execution
    let t = Instant::now();
    let run_result = command.wasi_cli_run().call_run(&mut store).map_err(|e| e.to_string())?;
    let run_us = t.elapsed().as_micros();

    if run_result.is_err() {
        return Err("component exited with error".to_string());
    }

    let total_us = total_start.elapsed().as_micros();
    let stdout = String::from_utf8(stdout_pipe.contents().to_vec()).map_err(|e| e.to_string())?;
    let stderr = String::from_utf8(stderr_pipe.contents().to_vec()).map_err(|e| e.to_string())?;

    Ok(WasmMetrics {
        stdout,
        stderr,
        wasm_size,
        engine_us,
        compile_us,
        link_us,
        instantiate_us,
        run_us,
        total_us,
    })
}

// --- Streaming stdout via channel ---

struct ChannelWriter {
    tx: std::sync::mpsc::Sender<Vec<u8>>,
}

impl HostOutputStream for ChannelWriter {
    fn write(&mut self, bytes: Bytes) -> StreamResult<()> {
        self.tx.send(bytes.to_vec()).map_err(|_| wasmtime_wasi::StreamError::Closed)?;
        Ok(())
    }
    fn flush(&mut self) -> StreamResult<()> {
        Ok(())
    }
    fn check_write(&mut self) -> StreamResult<usize> {
        Ok(1024 * 1024)
    }
}

#[async_trait::async_trait]
impl wasmtime_wasi::Subscribe for ChannelWriter {
    async fn ready(&mut self) {}
}

#[derive(Clone)]
struct ChannelStdout {
    tx: std::sync::mpsc::Sender<Vec<u8>>,
}

impl StdoutStream for ChannelStdout {
    fn stream(&self) -> Box<dyn HostOutputStream> {
        Box::new(ChannelWriter { tx: self.tx.clone() })
    }
    fn isatty(&self) -> bool {
        false
    }
}

/// Handle for cancelling a streaming WASI execution.
/// Cancels automatically on drop.
pub struct CancelHandle {
    engine: wasmtime::Engine,
}

impl CancelHandle {
    pub fn cancel(&self) {
        self.engine.increment_epoch();
    }
}

impl Drop for CancelHandle {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// Execute a WASI P2 component with streaming stdout.
///
/// Stdout is sent line-by-line through the returned receiver.
/// Call `CancelHandle::cancel()` to interrupt the component.
pub fn execute_wasm_streaming(
    config: &WasmConfig,
) -> Result<(std::sync::mpsc::Receiver<Vec<u8>>, CancelHandle, std::thread::JoinHandle<()>), String>
{
    use wasmtime::component::{Component, Linker, ResourceTable};
    use wasmtime::{Config, Engine, Store};
    use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiView, bindings::sync::CommandPre, pipe::MemoryOutputPipe};

    struct State {
        ctx: WasiCtx,
        table: ResourceTable,
    }
    impl WasiView for State {
        fn ctx(&mut self) -> &mut WasiCtx { &mut self.ctx }
        fn table(&mut self) -> &mut ResourceTable { &mut self.table }
    }

    let mut eng_config = Config::new();
    eng_config.wasm_component_model(true);
    eng_config.epoch_interruption(true);
    let engine = Engine::new(&eng_config).map_err(|e| e.to_string())?;

    let component = Component::new(&engine, config.wasm_bytes).map_err(|e| e.to_string())?;

    let mut linker = Linker::<State>::new(&engine);
    wasmtime_wasi::add_to_linker_sync(&mut linker).map_err(|e| e.to_string())?;

    let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();

    let stderr_pipe = MemoryOutputPipe::new(64 * 1024);
    let mut builder = WasiCtxBuilder::new();
    builder.stdout(ChannelStdout { tx });
    builder.stderr(stderr_pipe);
    builder.args(config.args);
    builder.envs(config.env_vars);
    if config.allow_network {
        builder.inherit_network();
        builder.allow_udp(true);
        builder.allow_tcp(true);
    }
    for preopen in config.preopens {
        builder.preopened_dir(
            &preopen.host_path,
            &preopen.guest_path,
            wasmtime_wasi::DirPerms::all(),
            wasmtime_wasi::FilePerms::all(),
        ).map_err(|e| e.to_string())?;
    }

    let mut store = Store::new(&engine, State {
        ctx: builder.build(),
        table: ResourceTable::new(),
    });
    store.set_epoch_deadline(u64::MAX);

    let pre = CommandPre::new(
        linker.instantiate_pre(&component).map_err(|e| e.to_string())?
    ).map_err(|e| e.to_string())?;
    let command = pre.instantiate(&mut store).map_err(|e| e.to_string())?;

    let cancel = CancelHandle { engine: engine.clone() };

    let handle = std::thread::spawn(move || {
        let _ = command.wasi_cli_run().call_run(&mut store);
    });

    Ok((rx, cancel, handle))
}
