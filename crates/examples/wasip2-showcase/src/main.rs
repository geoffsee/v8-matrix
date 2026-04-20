use std::collections::HashMap;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

fn main() {
    println!("========================================");
    println!("  WASI Preview 2 Showcase");
    println!("========================================\n");

    cli_args();
    environment();
    wall_clock();
    monotonic_clock();
    random();
    computation();
    stderr_demo();

    println!("\n========================================");
    println!("  showcase complete");
    println!("========================================");
}

/// wasi:cli/args — inspect command-line arguments
fn cli_args() {
    println!("[wasi:cli/args]");
    let args: Vec<String> = std::env::args().collect();
    println!("  argc: {}", args.len());
    for (i, arg) in args.iter().enumerate() {
        println!("  argv[{i}]: {arg}");
    }
    println!();
}

/// wasi:cli/environment — read environment variables
fn environment() {
    println!("[wasi:cli/environment]");
    let vars: Vec<(String, String)> = std::env::vars().collect();
    println!("  {} variable(s) set", vars.len());
    for (key, val) in &vars {
        println!("  {key}={val}");
    }
    println!();
}

/// wasi:clocks/wall-clock — get real-world time
fn wall_clock() {
    println!("[wasi:clocks/wall-clock]");
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    println!("  unix epoch:  {}.{:09}s", now.as_secs(), now.subsec_nanos());

    let secs = now.as_secs();
    let days = secs / 86400;
    let hours = (secs % 86400) / 3600;
    let mins = (secs % 3600) / 60;
    let s = secs % 60;
    println!("  uptime:      {days}d {hours:02}h {mins:02}m {s:02}s since epoch");
    println!();
}

/// wasi:clocks/monotonic-clock — precise timing
fn monotonic_clock() {
    println!("[wasi:clocks/monotonic-clock]");

    // Time a tight loop
    let n: u64 = 10_000_000;
    let start = Instant::now();
    let mut acc: u64 = 0;
    for i in 0..n {
        acc = acc.wrapping_add(i).wrapping_mul(7).wrapping_add(3);
    }
    let elapsed = start.elapsed();
    println!("  {n} iterations of wrapping arithmetic");
    println!("  result:    {acc} (to prevent optimization)");
    println!("  elapsed:   {elapsed:?}");
    println!("  per iter:  {:.1} ns", elapsed.as_nanos() as f64 / n as f64);
    println!();
}

/// wasi:random — cryptographic randomness
fn random() {
    println!("[wasi:random]");

    // Generate random bytes via HashMap's random state (uses wasi:random internally)
    // We use a trick: HashMap's hasher is seeded from wasi:random
    let mut entropy = Vec::new();
    for _ in 0..4 {
        let map: HashMap<u64, ()> = HashMap::new();
        // The hash builder contains random state; we extract it by hashing a known value
        use std::hash::{BuildHasher, Hasher};
        let mut hasher = map.hasher().build_hasher();
        hasher.write_u64(0);
        entropy.push(hasher.finish());
    }
    println!("  random u64s (from HashMap seeded hasher):");
    for (i, val) in entropy.iter().enumerate() {
        println!("    [{i}] {val:#018x}");
    }

    // Monte Carlo pi estimation using hash-derived randomness
    let samples = 50_000;
    let mut inside = 0u64;
    let start = Instant::now();
    for i in 0..samples {
        let map: HashMap<u64, ()> = HashMap::new();
        use std::hash::{BuildHasher, Hasher};
        let mut h = map.hasher().build_hasher();
        h.write_u64(i);
        let hash = h.finish();
        let x = (hash & 0xFFFFFFFF) as f64 / u32::MAX as f64;
        let y = (hash >> 32) as f64 / u32::MAX as f64;
        if x * x + y * y <= 1.0 {
            inside += 1;
        }
    }
    let pi = 4.0 * inside as f64 / samples as f64;
    let err = (pi - std::f64::consts::PI).abs();
    let elapsed = start.elapsed();
    println!("\n  monte carlo pi estimation ({samples} samples):");
    println!("    estimate:  {pi:.8}");
    println!("    actual:    {:.8}", std::f64::consts::PI);
    println!("    error:     {err:.8}");
    println!("    elapsed:   {elapsed:?}");
    println!();
}

/// Pure computation — sorting, searching, string manipulation
fn computation() {
    println!("[computation]");

    // Generate and sort data
    let n = 10_000;
    let start = Instant::now();
    let mut data: Vec<u32> = (0..n).rev().collect();
    let sort_start = Instant::now();
    data.sort();
    let sort_time = sort_start.elapsed();
    assert_eq!(data[0], 0);
    assert_eq!(data[n as usize - 1], n - 1);
    println!("  sorted {n} u32s in {sort_time:?}");

    // Binary search
    let search_start = Instant::now();
    let searches = 100_000;
    let mut found = 0;
    for i in 0..searches {
        if data.binary_search(&(i % n)).is_ok() {
            found += 1;
        }
    }
    let search_time = search_start.elapsed();
    println!("  {searches} binary searches ({found} hits) in {search_time:?}");

    // String operations
    let string_start = Instant::now();
    let mut s = String::new();
    for i in 0..1_000 {
        s.push_str(&format!("{i:04x}"));
    }
    let len = s.len();
    let upper = s.to_uppercase();
    let words: Vec<&str> = upper.as_bytes().chunks(4).filter_map(|c| std::str::from_utf8(c).ok()).collect();
    let string_time = string_start.elapsed();
    println!("  built {len}-char hex string, split into {} 4-char words in {string_time:?}", words.len());

    // Fibonacci with memoization
    let fib_start = Instant::now();
    let mut memo = vec![0u128; 187];
    memo[0] = 0;
    memo[1] = 1;
    for i in 2..187 {
        memo[i] = memo[i - 1] + memo[i - 2];
    }
    let fib_time = fib_start.elapsed();
    println!("  fib(186) = {} ({fib_time:?})", memo[186]);

    let total = start.elapsed();
    println!("  total computation: {total:?}");
    println!();
}

/// wasi:cli/stderr — write diagnostic output
fn stderr_demo() {
    eprintln!("[wasi:cli/stderr]");
    eprintln!("  this output was written to stderr");
    eprintln!("  useful for diagnostics without polluting stdout");
}
