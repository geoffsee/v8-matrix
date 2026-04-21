//! JS Runtime — Cloudflare Workers-compatible fetcher execution engine on V8.
//!
//! Provides a sandboxed JavaScript execution environment with:
//! - `globalThis.fetch` backed by native HTTP (reqwest)
//! - `Request` / `Response` / `Headers` Web API surface
//! - `TextEncoder` / `TextDecoder`
//! - `console.log/warn/error/...`
//! - `setTimeout` / `clearTimeout`
//!
//! User workloads implement the fetcher pattern:
//! ```js
//! export default {
//!   async fetch(request, env, ctx) {
//!     const resp = await fetch(env.API_BASE + "/data");
//!     return new Response(await resp.text());
//!   }
//! }
//! ```

pub mod bindings;
pub mod event_loop;

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Instant;

use event_loop::{OpState, run_event_loop};
use serde::{Deserialize, Serialize};

// ─── Bootstrap JS sources (compiled into the binary) ──────────────────────────

const BOOTSTRAP_PRIMORDIALS: &str = include_str!("bootstrap/00_primordials.js");
const BOOTSTRAP_FETCH: &str = include_str!("bootstrap/01_fetch.js");
const BOOTSTRAP_ENCODING: &str = include_str!("bootstrap/02_encoding.js");
const BOOTSTRAP_CONSOLE: &str = include_str!("bootstrap/03_console.js");

// ─── setTimeout / clearTimeout shim (wraps native bindings) ──────────────────

const TIMER_SHIM: &str = r#"
((globalThis) => {
  "use strict";
  const { ObjectDefineProperty } = globalThis.__primordials;

  ObjectDefineProperty(globalThis, "setTimeout", {
    value: function setTimeout(callback, delay) {
      if (typeof callback !== "function") {
        throw new TypeError("setTimeout: first argument must be a function");
      }
      return __native_set_timeout(callback, delay || 0);
    },
    writable: false,
    enumerable: false,
    configurable: false,
  });

  ObjectDefineProperty(globalThis, "clearTimeout", {
    value: function clearTimeout(id) {
      __native_clear_timeout(id || 0);
    },
    writable: false,
    enumerable: false,
    configurable: false,
  });
})(globalThis);
"#;

// ─── Public types ─────────────────────────────────────────────────────────────

/// A value that can be bound to `env.<name>` in the worker.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum BindingValue {
    /// Plain string: `env.API_KEY` → `"sk-..."`
    Text(String),
    /// JSON value: `env.CONFIG` → `{ "retries": 3 }`
    Json(serde_json::Value),
}

/// Incoming request passed to the worker's `fetch(request, env, ctx)`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsRequest {
    pub url: String,
    #[serde(default = "default_method")]
    pub method: String,
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    #[serde(default)]
    pub body: Option<String>,
}

fn default_method() -> String {
    "GET".into()
}

/// Response returned from the worker's `fetch()` handler.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

/// Configuration for a fetcher workload execution.
#[derive(Debug, Clone)]
pub struct FetcherConfig {
    /// The user's JavaScript module source code.
    pub script: String,
    /// Incoming request to pass as the first argument.
    pub request: JsRequest,
    /// Environment bindings accessible as `env.<key>`.
    pub bindings: HashMap<String, BindingValue>,
    /// Resource limits for the execution.
    pub limits: ResourceLimits,
}

/// Resource limits for a single execution.
#[derive(Debug, Clone)]
pub struct ResourceLimits {
    /// Maximum wall-clock time in milliseconds.
    pub max_duration_ms: u64,
    /// Maximum heap size in bytes (0 = default ~256MB).
    pub max_heap_bytes: usize,
    /// Maximum event loop iterations.
    pub max_event_loop_iterations: usize,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            max_duration_ms: 30_000,
            max_heap_bytes: 256 * 1024 * 1024,
            max_event_loop_iterations: 10_000,
        }
    }
}

/// Detailed metrics from a fetcher execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetcherMetrics {
    /// Time to set up the V8 isolate and bootstrap globals (microseconds).
    pub setup_us: u128,
    /// Time to compile and evaluate the user script (microseconds).
    pub compile_us: u128,
    /// Time spent in the event loop executing the fetch handler (microseconds).
    pub execute_us: u128,
    /// Total wall-clock time (microseconds).
    pub total_us: u128,
    /// Number of outbound fetch calls made by the script.
    pub fetch_count: usize,
    /// Number of event loop iterations.
    pub event_loop_iterations: usize,
    /// Console output collected: (level, message).
    pub console_output: Vec<(String, String)>,
}

/// The result of a fetcher execution.
pub struct FetcherResult {
    pub response: JsResponse,
    pub metrics: FetcherMetrics,
}

// ─── Runtime ──────────────────────────────────────────────────────────────────

/// Execute a fetcher workload in a fresh V8 isolate.
///
/// This is the primary entry point. Each call creates a new sandboxed V8
/// isolate, installs the Web API surface, runs the user's script, calls
/// its `default.fetch(request, env, ctx)` export, drives the event loop
/// to completion, and returns the Response.
pub fn execute_fetcher(config: &FetcherConfig) -> Result<FetcherResult, String> {
    crate::init_v8();

    let total_start = Instant::now();

    // We need a tokio runtime for async fetch operations.
    // Try to use the current runtime, or create a new one.
    let rt = tokio::runtime::Handle::try_current().unwrap_or_else(|_| {
        // No current runtime — create one. We leak it intentionally so
        // the handle remains valid for the isolate's lifetime.
        let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
        let handle = rt.handle().clone();
        std::mem::forget(rt);
        handle
    });

    let http_client = reqwest::Client::builder()
        .user_agent("v8-matrix/0.1")
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {e}"))?;

    let op_state = Rc::new(RefCell::new(OpState::new(rt, http_client)));

    // ── Create isolate ────────────────────────────────────────────────────

    let setup_start = Instant::now();

    let mut params = v8::CreateParams::default();
    if config.limits.max_heap_bytes > 0 {
        params = params.heap_limits(0, config.limits.max_heap_bytes);
    }

    let mut isolate = v8::Isolate::new(params);

    // Store OpState in the isolate slot so bindings can access it
    isolate.set_slot(op_state.clone());

    // Create the global object template and install native bindings,
    // then create the context with that template.
    let context = {
        let scope = &mut v8::HandleScope::new(&mut *isolate);
        let template = v8::ObjectTemplate::new(scope);
        bindings::install_bindings(scope, template);
        let options = v8::ContextOptions {
            global_template: Some(template),
            ..Default::default()
        };
        let ctx = v8::Context::new(scope, options);
        v8::Global::new(scope, ctx)
    };

    // ── Create context scope and run bootstrap ────────────────────────────

    let handle_scope = &mut v8::HandleScope::new(&mut *isolate);
    let context_local = v8::Local::new(handle_scope, &context);
    let scope = &mut v8::ContextScope::new(handle_scope, context_local);

    // Run bootstrap scripts in order
    run_script(scope, BOOTSTRAP_PRIMORDIALS, "00_primordials.js")?;
    run_script(scope, BOOTSTRAP_FETCH, "01_fetch.js")?;
    run_script(scope, BOOTSTRAP_ENCODING, "02_encoding.js")?;
    run_script(scope, BOOTSTRAP_CONSOLE, "03_console.js")?;
    run_script(scope, TIMER_SHIM, "timer_shim.js")?;

    let setup_us = setup_start.elapsed().as_micros();

    // ── Compile and run user script ───────────────────────────────────────

    let compile_start = Instant::now();

    // Wrap the user's module in an IIFE that captures the default export.
    // The user writes `export default { async fetch(request, env, ctx) { ... } }`
    // We transform it to extract the handler.
    let wrapped_script = format!(
        r#"
        (function() {{
            const __module = {{}};
            const __exports = {{}};
            (function(module, exports) {{
                {user_script}
            }})(__module, __exports);

            // Support multiple export styles:
            // 1. `export default {{ fetch() {{ }} }}` → rewritten to `__exports.default = ...`
            //    But since we can't use ES modules in v8::Script, we support:
            // 2. `module.exports = {{ async fetch() {{ }} }}`
            // 3. `exports.default = {{ async fetch() {{ }} }}`
            // 4. Just return an object with a fetch method from the IIFE
            return __module.exports || __exports.default || __exports;
        }})()
        "#,
        user_script = config.script
    );

    let module_value = run_script(scope, &wrapped_script, "user_script.js")?;
    let compile_us = compile_start.elapsed().as_micros();

    // ── Extract the fetch handler ─────────────────────────────────────────

    let module_obj = module_value
        .to_object(scope)
        .ok_or("Worker module did not return an object")?;

    let fetch_key = v8::String::new(scope, "fetch").unwrap();
    let fetch_val = module_obj
        .get(scope, fetch_key.into())
        .ok_or("Worker module has no 'fetch' export")?;

    let fetch_fn = v8::Local::<v8::Function>::try_from(fetch_val)
        .map_err(|_| "Worker 'fetch' export is not a function")?;

    // ── Build (request, env, ctx) arguments ───────────────────────────────

    let request_obj = build_request_object(scope, &config.request)?;
    let env_obj = build_env_object(scope, &config.bindings)?;
    let ctx_obj = build_ctx_object(scope)?;

    // ── Call fetch(request, env, ctx) ─────────────────────────────────────

    let execute_start = Instant::now();

    let result = {
        let tc = &mut v8::TryCatch::new(scope);
        let result = fetch_fn.call(
            tc,
            module_obj.into(),
            &[request_obj.into(), env_obj.into(), ctx_obj.into()],
        );
        match result {
            Some(val) => val,
            None => {
                let msg = if let Some(exc) = tc.exception() {
                    exc.to_rust_string_lossy(tc)
                } else {
                    "fetch() threw an exception".to_string()
                };
                return Err(msg);
            }
        }
    };

    // ── Drive event loop if the result is a Promise ───────────────────────

    let response_value = if result.is_promise() {
        let promise = v8::Local::<v8::Promise>::try_from(result)
            .map_err(|_| "Expected a Promise from fetch()")?;
        let promise_global = v8::Global::new(scope, promise);

        run_event_loop(scope, &promise_global, config.limits.max_event_loop_iterations)?;

        let settled = v8::Local::new(scope, &promise_global);
        match settled.state() {
            v8::PromiseState::Fulfilled => settled.result(scope),
            v8::PromiseState::Rejected => {
                let reason = settled.result(scope);
                return Err(format!("fetch() rejected: {}", reason.to_rust_string_lossy(scope)));
            }
            v8::PromiseState::Pending => {
                return Err("fetch() promise never resolved".into());
            }
        }
    } else {
        result
    };

    let execute_us = execute_start.elapsed().as_micros();

    // ── Extract the Response ──────────────────────────────────────────────

    let response = extract_response(scope, response_value)?;

    let total_us = total_start.elapsed().as_micros();

    // Collect metrics
    let state = op_state.borrow();
    let metrics = FetcherMetrics {
        setup_us,
        compile_us,
        execute_us,
        total_us,
        fetch_count: 0, // TODO: track in op_state
        event_loop_iterations: 0, // TODO: track in event_loop
        console_output: state.console_output.clone(),
    };

    Ok(FetcherResult { response, metrics })
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

/// Compile and run a script, returning its result value.
fn run_script<'s>(
    scope: &mut v8::HandleScope<'s>,
    source: &str,
    filename: &str,
) -> Result<v8::Local<'s, v8::Value>, String> {
    let code = v8::String::new(scope, source)
        .ok_or_else(|| format!("Failed to create source string for {filename}"))?;

    let origin = create_script_origin(scope, filename);

    let tc = &mut v8::TryCatch::new(scope);

    let script = v8::Script::compile(tc, code, Some(&origin))
        .ok_or_else(|| {
            if let Some(exc) = tc.exception() {
                format!("{filename}: {}", exc.to_rust_string_lossy(tc))
            } else {
                format!("Failed to compile {filename}")
            }
        })?;

    script.run(tc).ok_or_else(|| {
        if let Some(exc) = tc.exception() {
            format!("{filename}: {}", exc.to_rust_string_lossy(tc))
        } else {
            format!("{filename}: execution failed")
        }
    })
}

fn create_script_origin<'s>(
    scope: &mut v8::HandleScope<'s>,
    filename: &str,
) -> v8::ScriptOrigin<'s> {
    let name = v8::String::new(scope, filename).unwrap();
    v8::ScriptOrigin::new(
        scope,
        name.into(),
        0,     // line offset
        0,     // column offset
        false, // is shared cross origin
        -1,    // script id
        None,  // source map url
        false, // is opaque
        false, // is wasm
        false, // is module
        None,  // host defined options
    )
}

/// Build a JS `Request` object from JsRequest.
fn build_request_object<'s>(
    scope: &mut v8::HandleScope<'s>,
    req: &JsRequest,
) -> Result<v8::Local<'s, v8::Object>, String> {
    // Get the Request constructor from globalThis
    let global = scope.get_current_context().global(scope);
    let request_key = v8::String::new(scope, "Request").unwrap();
    let request_ctor = global
        .get(scope, request_key.into())
        .ok_or("Request constructor not found")?;
    let request_ctor = v8::Local::<v8::Function>::try_from(request_ctor)
        .map_err(|_| "Request is not a constructor")?;

    // Build init object
    let init = v8::Object::new(scope);

    let method_key = v8::String::new(scope, "method").unwrap();
    let method_val = v8::String::new(scope, &req.method).unwrap();
    init.set(scope, method_key.into(), method_val.into());

    // Build headers object
    let headers_obj = v8::Object::new(scope);
    for (k, v) in &req.headers {
        let key = v8::String::new(scope, k).unwrap();
        let val = v8::String::new(scope, v).unwrap();
        headers_obj.set(scope, key.into(), val.into());
    }
    let headers_key = v8::String::new(scope, "headers").unwrap();
    init.set(scope, headers_key.into(), headers_obj.into());

    if let Some(ref body) = req.body {
        let body_key = v8::String::new(scope, "body").unwrap();
        let body_val = v8::String::new(scope, body).unwrap();
        init.set(scope, body_key.into(), body_val.into());
    }

    let url = v8::String::new(scope, &req.url).unwrap();

    let result = request_ctor
        .new_instance(scope, &[url.into(), init.into()])
        .ok_or("Failed to construct Request object")?;

    Ok(result)
}

/// Build a frozen `env` object from bindings.
fn build_env_object<'s>(
    scope: &mut v8::HandleScope<'s>,
    bindings: &HashMap<String, BindingValue>,
) -> Result<v8::Local<'s, v8::Object>, String> {
    let obj = v8::Object::new(scope);

    for (key, value) in bindings {
        let k = v8::String::new(scope, key).unwrap();
        let v: v8::Local<v8::Value> = match value {
            BindingValue::Text(s) => v8::String::new(scope, s).unwrap().into(),
            BindingValue::Json(json_val) => {
                json_to_v8(scope, json_val)?
            }
        };
        obj.set(scope, k.into(), v);
    }

    // Freeze the env object
    let global = scope.get_current_context().global(scope);
    let object_key = v8::String::new(scope, "Object").unwrap();
    let object_val = global.get(scope, object_key.into()).unwrap();
    let object_obj = v8::Local::<v8::Object>::try_from(object_val).unwrap();
    let freeze_key = v8::String::new(scope, "freeze").unwrap();
    let freeze_fn = object_obj.get(scope, freeze_key.into()).unwrap();
    let freeze_fn = v8::Local::<v8::Function>::try_from(freeze_fn).unwrap();

    let undefined = v8::undefined(scope);
    freeze_fn.call(scope, undefined.into(), &[obj.into()]);

    Ok(obj)
}

/// Build the `ctx` execution context object.
fn build_ctx_object<'s>(
    scope: &mut v8::HandleScope<'s>,
) -> Result<v8::Local<'s, v8::Object>, String> {
    let obj = v8::Object::new(scope);

    // ctx.waitUntil(promise) — currently a no-op that accepts the promise
    let wait_until_fn = v8::Function::new(scope, |_scope: &mut v8::HandleScope,
        _args: v8::FunctionCallbackArguments,
        _rv: v8::ReturnValue| {
        // In a full implementation this would extend the execution lifetime.
        // For now, accept the promise but don't block on it.
    }).unwrap();
    let key = v8::String::new(scope, "waitUntil").unwrap();
    obj.set(scope, key.into(), wait_until_fn.into());

    // ctx.passThroughOnException() — no-op stub
    let passthrough_fn = v8::Function::new(scope, |_scope: &mut v8::HandleScope,
        _args: v8::FunctionCallbackArguments,
        _rv: v8::ReturnValue| {
        // No-op in this runtime
    }).unwrap();
    let key = v8::String::new(scope, "passThroughOnException").unwrap();
    obj.set(scope, key.into(), passthrough_fn.into());

    Ok(obj)
}

/// Extract a JsResponse from a V8 Response object.
fn extract_response(
    scope: &mut v8::HandleScope,
    value: v8::Local<v8::Value>,
) -> Result<JsResponse, String> {
    let obj = value
        .to_object(scope)
        .ok_or("fetch() did not return an object")?;

    // status
    let status_key = v8::String::new(scope, "status").unwrap();
    let status = obj
        .get(scope, status_key.into())
        .and_then(|v| v.uint32_value(scope))
        .unwrap_or(200) as u16;

    // headers — call the Headers iterator
    let headers_key = v8::String::new(scope, "headers").unwrap();
    let headers_val = obj.get(scope, headers_key.into());
    let mut headers = Vec::new();

    if let Some(headers_val) = headers_val {
        if let Ok(headers_obj) = v8::Local::<v8::Object>::try_from(headers_val) {
            // Try to call headers.entries() to iterate
            let entries_key = v8::String::new(scope, "entries").unwrap();
            if let Some(entries_fn) = headers_obj.get(scope, entries_key.into()) {
                if let Ok(entries_fn) = v8::Local::<v8::Function>::try_from(entries_fn) {
                    if let Some(iter) = entries_fn.call(scope, headers_obj.into(), &[]) {
                        if let Ok(iter_obj) = v8::Local::<v8::Object>::try_from(iter) {
                            let next_key = v8::String::new(scope, "next").unwrap();
                            if let Some(next_fn) = iter_obj.get(scope, next_key.into()) {
                                if let Ok(next_fn) = v8::Local::<v8::Function>::try_from(next_fn) {
                                    loop {
                                        let result = next_fn.call(scope, iter_obj.into(), &[]);
                                        if let Some(result) = result {
                                            if let Ok(result_obj) = v8::Local::<v8::Object>::try_from(result) {
                                                let done_key = v8::String::new(scope, "done").unwrap();
                                                if let Some(done) = result_obj.get(scope, done_key.into()) {
                                                    if done.is_true() {
                                                        break;
                                                    }
                                                }
                                                let value_key = v8::String::new(scope, "value").unwrap();
                                                if let Some(pair) = result_obj.get(scope, value_key.into()) {
                                                    if let Ok(pair_obj) = v8::Local::<v8::Array>::try_from(pair) {
                                                        if pair_obj.length() >= 2 {
                                                            let k = pair_obj.get_index(scope, 0).unwrap().to_rust_string_lossy(scope);
                                                            let v = pair_obj.get_index(scope, 1).unwrap().to_rust_string_lossy(scope);
                                                            headers.push((k, v));
                                                        }
                                                    }
                                                }
                                            } else {
                                                break;
                                            }
                                        } else {
                                            break;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // body — call .text() which returns a Promise, then resolve synchronously
    // Since Response stores the body internally, we can access the private body field
    // by calling text() and driving the microtask queue
    let body = extract_response_body(scope, obj)?;

    Ok(JsResponse {
        status,
        headers,
        body,
    })
}

/// Extract the body from a Response object by calling .text().
fn extract_response_body(
    scope: &mut v8::HandleScope,
    response_obj: v8::Local<v8::Object>,
) -> Result<String, String> {
    let text_key = v8::String::new(scope, "text").unwrap();
    let text_fn = response_obj
        .get(scope, text_key.into())
        .ok_or("Response has no text() method")?;

    let text_fn = v8::Local::<v8::Function>::try_from(text_fn)
        .map_err(|_| "Response.text is not a function")?;

    let result = text_fn.call(scope, response_obj.into(), &[])
        .ok_or("Response.text() call failed")?;

    if result.is_promise() {
        let promise = v8::Local::<v8::Promise>::try_from(result)
            .map_err(|_| "Expected Promise from text()")?;

        // text() is synchronous internally (no I/O), so one microtask checkpoint suffices
        scope.perform_microtask_checkpoint();

        match promise.state() {
            v8::PromiseState::Fulfilled => {
                Ok(promise.result(scope).to_rust_string_lossy(scope))
            }
            v8::PromiseState::Rejected => {
                Err(format!(
                    "Response.text() rejected: {}",
                    promise.result(scope).to_rust_string_lossy(scope)
                ))
            }
            v8::PromiseState::Pending => {
                // Try a few more checkpoints
                for _ in 0..10 {
                    scope.perform_microtask_checkpoint();
                    if promise.state() != v8::PromiseState::Pending {
                        break;
                    }
                }
                match promise.state() {
                    v8::PromiseState::Fulfilled => {
                        Ok(promise.result(scope).to_rust_string_lossy(scope))
                    }
                    _ => Err("Response.text() never resolved".into()),
                }
            }
        }
    } else {
        Ok(result.to_rust_string_lossy(scope))
    }
}

/// Convert a serde_json::Value to a V8 value.
fn json_to_v8<'s>(
    scope: &mut v8::HandleScope<'s>,
    value: &serde_json::Value,
) -> Result<v8::Local<'s, v8::Value>, String> {
    match value {
        serde_json::Value::Null => Ok(v8::null(scope).into()),
        serde_json::Value::Bool(b) => Ok(v8::Boolean::new(scope, *b).into()),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(v8::Number::new(scope, i as f64).into())
            } else if let Some(f) = n.as_f64() {
                Ok(v8::Number::new(scope, f).into())
            } else {
                Err("Unsupported number value".into())
            }
        }
        serde_json::Value::String(s) => {
            Ok(v8::String::new(scope, s).unwrap().into())
        }
        serde_json::Value::Array(arr) => {
            let v8_arr = v8::Array::new(scope, arr.len() as i32);
            for (i, item) in arr.iter().enumerate() {
                let v = json_to_v8(scope, item)?;
                v8_arr.set_index(scope, i as u32, v);
            }
            Ok(v8_arr.into())
        }
        serde_json::Value::Object(map) => {
            let obj = v8::Object::new(scope);
            for (key, val) in map {
                let k = v8::String::new(scope, key).unwrap();
                let v = json_to_v8(scope, val)?;
                obj.set(scope, k.into(), v);
            }
            Ok(obj.into())
        }
    }
}
