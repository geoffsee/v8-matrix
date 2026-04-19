fn main() {
    // Initialize V8
    let platform = v8::new_default_platform(0, false).make_shared();
    v8::V8::initialize_platform(platform);
    v8::V8::initialize();

    // Create an isolate (a V8 VM instance)
    let isolate = &mut v8::Isolate::new(v8::CreateParams::default());

    // Create a handle scope
    let handle_scope = &mut v8::HandleScope::new(isolate);

    // Create a context
    let context = v8::Context::new(handle_scope, Default::default());
    let scope = &mut v8::ContextScope::new(handle_scope, context);

    // Compile and run a JavaScript snippet
    let code = v8::String::new(scope, "1 + 2 + ' hello from V8!'").unwrap();
    let script = v8::Script::compile(scope, code, None).unwrap();
    let result = script.run(scope).unwrap();

    // Convert the result to a Rust string and print it
    let result_str = result.to_string(scope).unwrap();
    println!("JS result: {}", result_str.to_rust_string_lossy(scope));
}
