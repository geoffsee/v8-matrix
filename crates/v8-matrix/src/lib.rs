use std::sync::Once;

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

/// Execute a WASI P2 component module.
///
/// Takes raw `.wasm` component bytes and CLI args.
/// Runs the component's `wasi:cli/run` entry point, captures stdout,
/// and returns metrics for each phase.
pub fn execute_wasm(wasm_bytes: &[u8], args: &[String]) -> Result<WasmMetrics, String> {
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
    let wasm_size = wasm_bytes.len();

    // Engine creation
    let t = Instant::now();
    let mut config = Config::new();
    config.wasm_component_model(true);
    let engine = Engine::new(&config).map_err(|e| e.to_string())?;
    let engine_us = t.elapsed().as_micros();

    // Component compilation
    let t = Instant::now();
    let component = Component::new(&engine, wasm_bytes).map_err(|e| e.to_string())?;
    let compile_us = t.elapsed().as_micros();

    // Linking
    let t = Instant::now();
    let mut linker = Linker::<State>::new(&engine);
    wasmtime_wasi::add_to_linker_sync(&mut linker).map_err(|e| e.to_string())?;
    let link_us = t.elapsed().as_micros();

    // Store + WASI context setup
    let stdout = MemoryOutputPipe::new(4096);
    let mut builder = WasiCtxBuilder::new();
    builder.stdout(stdout.clone());
    builder.args(args);
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
    let output = String::from_utf8(stdout.contents().to_vec()).map_err(|e| e.to_string())?;

    Ok(WasmMetrics {
        stdout: output,
        wasm_size,
        engine_us,
        compile_us,
        link_us,
        instantiate_us,
        run_us,
        total_us,
    })
}
