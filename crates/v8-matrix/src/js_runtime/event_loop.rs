//! Event loop — drives V8 microtasks and pending async operations to completion.
//!
//! The loop follows the same basic model as Deno:
//!   1. Drain the V8 microtask queue
//!   2. Check if the top-level promise has settled → done
//!   3. Poll pending ops (HTTP requests, timers) on the tokio runtime
//!   4. Resolve/reject corresponding V8 promises
//!   5. Goto 1

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;
use std::time::Instant;

/// Tracks all pending async operations and runtime state.
pub struct OpState {
    /// Pending operations waiting to be driven
    pub pending_ops: Vec<PendingOp>,
    /// Console output collected during execution: (level, message)
    pub console_output: Vec<(String, String)>,
    /// Timer ID counter
    pub next_timer_id: u32,
    /// Cancelled timer IDs
    pub cancelled_timers: HashSet<u32>,
    /// Tokio runtime handle for spawning async work
    pub tokio_handle: tokio::runtime::Handle,
    /// HTTP client reused across fetch calls
    pub http_client: reqwest::Client,
}

impl OpState {
    pub fn new(tokio_handle: tokio::runtime::Handle, http_client: reqwest::Client) -> Self {
        Self {
            pending_ops: Vec::new(),
            console_output: Vec::new(),
            next_timer_id: 1,
            cancelled_timers: HashSet::new(),
            tokio_handle,
            http_client,
        }
    }
}

/// A single pending async operation.
pub enum PendingOp {
    Fetch {
        url: String,
        method: String,
        headers: Vec<(String, String)>,
        body: Option<String>,
        resolver: v8::Global<v8::PromiseResolver>,
    },
    Timer {
        id: u32,
        deadline: Instant,
        callback: v8::Global<v8::Function>,
    },
}

/// Result of a completed fetch operation, ready to be resolved in V8.
struct FetchResult {
    resolver: v8::Global<v8::PromiseResolver>,
    outcome: Result<FetchResponse, String>,
}

struct FetchResponse {
    status: u16,
    status_text: String,
    headers: Vec<(String, String)>,
    body: String,
}

/// Result of a completed timer, ready to fire its callback.
struct TimerResult {
    callback: v8::Global<v8::Function>,
}

/// Run the event loop until the given promise settles or an error occurs.
///
/// `max_iterations` prevents infinite loops in degenerate cases.
pub fn run_event_loop(
    scope: &mut v8::HandleScope,
    promise: &v8::Global<v8::Promise>,
    max_iterations: usize,
) -> Result<(), String> {
    for _ in 0..max_iterations {
        // 1. Drain microtask queue
        scope.perform_microtask_checkpoint();

        // 2. Check if the top-level promise settled
        let local_promise = v8::Local::new(scope, promise);
        match local_promise.state() {
            v8::PromiseState::Fulfilled | v8::PromiseState::Rejected => {
                return Ok(());
            }
            v8::PromiseState::Pending => {}
        }

        // 3. Drain pending ops
        let op_state = scope
            .get_slot::<Rc<RefCell<OpState>>>()
            .expect("OpState missing")
            .clone();

        let ops: Vec<PendingOp> = {
            let mut state = op_state.borrow_mut();
            std::mem::take(&mut state.pending_ops)
        };

        if ops.is_empty() {
            // No pending ops and promise not settled — this shouldn't happen
            // in well-formed code, but avoid spinning. Give V8 one more checkpoint.
            scope.perform_microtask_checkpoint();
            let local_promise = v8::Local::new(scope, promise);
            if local_promise.state() != v8::PromiseState::Pending {
                return Ok(());
            }
            return Err("Event loop stalled: promise pending with no ops".into());
        }

        // Separate fetches and timers
        let mut fetch_ops = Vec::new();
        let mut timer_ops = Vec::new();

        {
            let state = op_state.borrow();
            for op in ops {
                match op {
                    PendingOp::Timer { id, .. } if state.cancelled_timers.contains(&id) => {
                        // Skip cancelled timers
                    }
                    PendingOp::Fetch { .. } => fetch_ops.push(op),
                    PendingOp::Timer { .. } => timer_ops.push(op),
                }
            }
        }

        // 4a. Execute fetch ops on tokio
        let fetch_results = execute_fetches(&op_state, fetch_ops);

        // 4b. Execute ready timers
        let timer_results = execute_timers(&op_state, timer_ops);

        // 5. Resolve fetch promises in V8
        for result in fetch_results {
            let resolver = v8::Local::new(scope, &result.resolver);
            match result.outcome {
                Ok(resp) => {
                    let obj = build_fetch_response_object(scope, &resp);
                    resolver.resolve(scope, obj.into());
                }
                Err(err) => {
                    let msg = v8::String::new(scope, &err).unwrap();
                    let exc = v8::Exception::error(scope, msg);
                    resolver.reject(scope, exc);
                }
            }
        }

        // 5b. Fire timer callbacks
        for result in timer_results {
            let callback = v8::Local::new(scope, &result.callback);
            let undefined = v8::undefined(scope);
            callback.call(scope, undefined.into(), &[]);
        }
    }

    Err(format!(
        "Event loop exceeded max iterations ({})",
        max_iterations
    ))
}

/// Execute pending fetch operations on tokio and collect results.
fn execute_fetches(
    op_state: &Rc<RefCell<OpState>>,
    ops: Vec<PendingOp>,
) -> Vec<FetchResult> {
    if ops.is_empty() {
        return Vec::new();
    }

    let state = op_state.borrow();
    let handle = state.tokio_handle.clone();
    let client = state.http_client.clone();
    drop(state);

    let mut results = Vec::with_capacity(ops.len());

    for op in ops {
        if let PendingOp::Fetch {
            url,
            method,
            headers,
            body,
            resolver,
        } = op
        {
            let client = client.clone();
            let outcome = handle.block_on(async move {
                do_fetch(&client, &url, &method, &headers, body.as_deref()).await
            });

            results.push(FetchResult { resolver, outcome });
        }
    }

    results
}

/// Perform the actual HTTP fetch.
async fn do_fetch(
    client: &reqwest::Client,
    url: &str,
    method: &str,
    headers: &[(String, String)],
    body: Option<&str>,
) -> Result<FetchResponse, String> {
    let method = reqwest::Method::from_bytes(method.as_bytes())
        .map_err(|e| format!("Invalid HTTP method: {e}"))?;

    let mut req = client.request(method, url);

    for (key, value) in headers {
        req = req.header(key.as_str(), value.as_str());
    }

    if let Some(body) = body {
        req = req.body(body.to_string());
    }

    let response = req.send().await.map_err(|e| format!("fetch failed: {e}"))?;

    let status = response.status().as_u16();
    let status_text = response
        .status()
        .canonical_reason()
        .unwrap_or("")
        .to_string();

    let resp_headers: Vec<(String, String)> = response
        .headers()
        .iter()
        .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string()))
        .collect();

    let body = response
        .text()
        .await
        .map_err(|e| format!("Failed to read response body: {e}"))?;

    Ok(FetchResponse {
        status,
        status_text,
        headers: resp_headers,
        body,
    })
}

/// Execute ready timers and return any that need to be re-queued.
fn execute_timers(
    op_state: &Rc<RefCell<OpState>>,
    ops: Vec<PendingOp>,
) -> Vec<TimerResult> {
    if ops.is_empty() {
        return Vec::new();
    }

    let now = Instant::now();
    let mut ready = Vec::new();
    let mut not_ready = Vec::new();

    for op in ops {
        if let PendingOp::Timer {
            id,
            deadline,
            callback,
        } = op
        {
            if deadline <= now {
                ready.push(TimerResult { callback });
            } else {
                not_ready.push(PendingOp::Timer {
                    id,
                    deadline,
                    callback,
                });
            }
        }
    }

    // If there are timers that aren't ready yet, sleep until the earliest one
    if !not_ready.is_empty() && ready.is_empty() {
        // Find the earliest deadline
        let earliest = not_ready
            .iter()
            .filter_map(|op| match op {
                PendingOp::Timer { deadline, .. } => Some(*deadline),
                _ => None,
            })
            .min()
            .unwrap();

        let sleep_duration = earliest.saturating_duration_since(Instant::now());
        if !sleep_duration.is_zero() {
            std::thread::sleep(sleep_duration);
        }

        // Now check again
        let now = Instant::now();
        let mut still_not_ready = Vec::new();
        for op in not_ready {
            if let PendingOp::Timer {
                id,
                deadline,
                callback,
            } = op
            {
                if deadline <= now {
                    ready.push(TimerResult { callback });
                } else {
                    still_not_ready.push(PendingOp::Timer {
                        id,
                        deadline,
                        callback,
                    });
                }
            }
        }
        not_ready = still_not_ready;
    }

    // Re-queue timers that still aren't ready
    if !not_ready.is_empty() {
        let mut state = op_state.borrow_mut();
        state.pending_ops.extend(not_ready);
    }

    ready
}

/// Build a JS object from a FetchResponse: { status, statusText, headers, body }
fn build_fetch_response_object<'s>(
    scope: &mut v8::HandleScope<'s>,
    resp: &FetchResponse,
) -> v8::Local<'s, v8::Object> {
    let obj = v8::Object::new(scope);

    // status
    let key = v8::String::new(scope, "status").unwrap();
    let val = v8::Number::new(scope, resp.status as f64);
    obj.set(scope, key.into(), val.into());

    // statusText
    let key = v8::String::new(scope, "statusText").unwrap();
    let val = v8::String::new(scope, &resp.status_text).unwrap();
    obj.set(scope, key.into(), val.into());

    // headers as [[key, value], ...]
    let key = v8::String::new(scope, "headers").unwrap();
    let arr = v8::Array::new(scope, resp.headers.len() as i32);
    for (i, (hk, hv)) in resp.headers.iter().enumerate() {
        let pair = v8::Array::new(scope, 2);
        let k = v8::String::new(scope, hk).unwrap();
        let v = v8::String::new(scope, hv).unwrap();
        pair.set_index(scope, 0, k.into());
        pair.set_index(scope, 1, v.into());
        arr.set_index(scope, i as u32, pair.into());
    }
    obj.set(scope, key.into(), arr.into());

    // body
    let key = v8::String::new(scope, "body").unwrap();
    let val = v8::String::new(scope, &resp.body).unwrap();
    obj.set(scope, key.into(), val.into());

    obj
}
