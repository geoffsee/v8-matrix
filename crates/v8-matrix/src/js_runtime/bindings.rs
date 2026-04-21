//! Native V8 function bindings — bridges JS calls to Rust implementations.
//!
//! Each binding is a `v8::FunctionCallback` installed on the global object
//! before user code runs. Async operations (like fetch) enqueue work onto the
//! `OpState` stored in the isolate slot, and the event loop polls them to
//! completion.

use std::cell::RefCell;
use std::rc::Rc;

use crate::js_runtime::event_loop::{OpState, PendingOp};

/// Install all native bindings on the given global object template.
pub fn install_bindings(
    scope: &mut v8::HandleScope<'_, ()>,
    global: v8::Local<v8::ObjectTemplate>,
) {
    set_func(scope, global, "__native_fetch", native_fetch);
    set_func(scope, global, "__native_console_log", native_console_log);
    set_func(scope, global, "__native_set_timeout", native_set_timeout);
    set_func(scope, global, "__native_clear_timeout", native_clear_timeout);
    set_func(scope, global, "__native_encode_utf8", native_encode_utf8);
    set_func(scope, global, "__native_decode_utf8", native_decode_utf8);
}

fn set_func(
    scope: &mut v8::HandleScope<'_, ()>,
    global: v8::Local<v8::ObjectTemplate>,
    name: &str,
    callback: impl v8::MapFnTo<v8::FunctionCallback>,
) {
    let key = v8::String::new(scope, name).unwrap();
    let val = v8::FunctionTemplate::new(scope, callback);
    global.set(key.into(), val.into());
}

// ─── Fetch ────────────────────────────────────────────────────────────────────

/// `__native_fetch(url, method, headersJson, body)` → Promise<{ status, statusText, headers, body }>
fn native_fetch(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let url = args.get(0).to_rust_string_lossy(scope);
    let method = args.get(1).to_rust_string_lossy(scope);
    let headers_json = args.get(2).to_rust_string_lossy(scope);
    let body = if args.get(3).is_null_or_undefined() {
        None
    } else {
        Some(args.get(3).to_rust_string_lossy(scope))
    };

    let resolver = v8::PromiseResolver::new(scope).unwrap();
    let promise = resolver.get_promise(scope);
    let resolver_global = v8::Global::new(scope, resolver);

    // Parse headers
    let headers: Vec<(String, String)> = serde_json::from_str(&headers_json).unwrap_or_default();

    let op = PendingOp::Fetch {
        url,
        method,
        headers,
        body,
        resolver: resolver_global,
    };

    // Get op_state from isolate slot and push the op
    let op_state = scope
        .get_slot::<Rc<RefCell<OpState>>>()
        .expect("OpState not found in isolate slot")
        .clone();
    op_state.borrow_mut().pending_ops.push(op);

    rv.set(promise.into());
}

// ─── Console ──────────────────────────────────────────────────────────────────

/// `__native_console_log(level, message)`
fn native_console_log(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    let level = args.get(0).to_rust_string_lossy(scope);
    let message = args.get(1).to_rust_string_lossy(scope);

    let op_state = scope
        .get_slot::<Rc<RefCell<OpState>>>()
        .expect("OpState not found in isolate slot")
        .clone();
    op_state.borrow_mut().console_output.push((level, message));
}

// ─── Timers ───────────────────────────────────────────────────────────────────

/// `__native_set_timeout(callback, delay_ms)` → timer_id
fn native_set_timeout(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let callback = v8::Local::<v8::Function>::try_from(args.get(0));
    let delay_ms = args.get(1).uint32_value(scope).unwrap_or(0) as u64;

    let callback = match callback {
        Ok(f) => f,
        Err(_) => {
            let msg = v8::String::new(scope, "setTimeout: first argument must be a function").unwrap();
            let exc = v8::Exception::type_error(scope, msg);
            scope.throw_exception(exc);
            return;
        }
    };

    let callback_global = v8::Global::new(scope, callback);

    let op_state = scope
        .get_slot::<Rc<RefCell<OpState>>>()
        .expect("OpState not found in isolate slot")
        .clone();

    let mut state = op_state.borrow_mut();
    let timer_id = state.next_timer_id;
    state.next_timer_id += 1;

    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(delay_ms);
    state.pending_ops.push(PendingOp::Timer {
        id: timer_id,
        deadline,
        callback: callback_global,
    });

    let id_val = v8::Number::new(scope, timer_id as f64);
    rv.set(id_val.into());
}

/// `__native_clear_timeout(timer_id)`
fn native_clear_timeout(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    let timer_id = args.get(0).uint32_value(scope).unwrap_or(0);

    let op_state = scope
        .get_slot::<Rc<RefCell<OpState>>>()
        .expect("OpState not found in isolate slot")
        .clone();
    op_state.borrow_mut().cancelled_timers.insert(timer_id);
}

// ─── Encoding ─────────────────────────────────────────────────────────────────

/// `__native_encode_utf8(string)` → Uint8Array
fn native_encode_utf8(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let input = args.get(0).to_rust_string_lossy(scope);
    let bytes = input.as_bytes();

    let len = bytes.len();
    let backing_store = v8::ArrayBuffer::new_backing_store_from_vec(bytes.to_vec());
    let ab = v8::ArrayBuffer::with_backing_store(scope, &backing_store.into());
    let ua = v8::Uint8Array::new(scope, ab, 0, len).unwrap();

    rv.set(ua.into());
}

/// `__native_decode_utf8(uint8array)` → string
fn native_decode_utf8(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let input = args.get(0);

    let ab_view = match v8::Local::<v8::ArrayBufferView>::try_from(input) {
        Ok(view) => view,
        Err(_) => {
            let msg = v8::String::new(scope, "decode_utf8: expected ArrayBufferView").unwrap();
            let exc = v8::Exception::type_error(scope, msg);
            scope.throw_exception(exc);
            return;
        }
    };

    let len = ab_view.byte_length();
    let mut buf = vec![0u8; len];
    ab_view.copy_contents(&mut buf);

    let s = match std::str::from_utf8(&buf) {
        Ok(s) => s,
        Err(_) => {
            let msg = v8::String::new(scope, "decode_utf8: invalid UTF-8").unwrap();
            let exc = v8::Exception::type_error(scope, msg);
            scope.throw_exception(exc);
            return;
        }
    };

    let v8_str = v8::String::new(scope, s).unwrap();
    rv.set(v8_str.into());
}
