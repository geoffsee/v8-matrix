use std::net::UdpSocket;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        return cmd_help();
    }

    // Log to history
    log_history(&args.join(" "));

    match args[0].as_str() {
        "help" => cmd_help(),
        "time" => cmd_time(),
        "env" => cmd_env(),
        "rand" => cmd_rand(args.get(1).and_then(|s| s.parse().ok()).unwrap_or(16)),
        "pi" => cmd_pi(args.get(1).and_then(|s| s.parse().ok()).unwrap_or(100_000)),
        "fib" => cmd_fib(args.get(1).and_then(|s| s.parse().ok()).unwrap_or(50)),
        "sort" => cmd_sort(args.get(1).and_then(|s| s.parse().ok()).unwrap_or(100_000)),
        "bench" => cmd_bench(),
        "echo" => cmd_echo(&args[1..].join(" ")),
        "set" => cmd_set(args.get(1).map(|s| s.as_str()).unwrap_or(""), &args[2..].join(" ")),
        "get" => cmd_get(args.get(1).map(|s| s.as_str()).unwrap_or("")),
        "del" => cmd_del(args.get(1).map(|s| s.as_str()).unwrap_or("")),
        "keys" => cmd_keys(),
        "history" => cmd_history(),
        "flood" => cmd_flood(args.get(1).and_then(|s| s.parse().ok()).unwrap_or(64)),
        other => {
            println!("unknown command: {other}");
            eprintln!("type 'help' for available commands");
        }
    }
}

fn cmd_help() {
    println!("available commands:");
    println!("  time           wall clock              wasi:clocks");
    println!("  env            environment vars         wasi:cli/environment");
    println!("  rand [n]       random bytes (def 16)    wasi:random");
    println!("  pi [samples]   monte carlo pi           wasi:random + clocks");
    println!("  fib [n]        fibonacci (def 50)       wasi:clocks");
    println!("  sort [n]       sort integers (def 100k) wasi:random + clocks");
    println!("  bench          run all benchmarks       wasi:clocks");
    println!("  echo <msg>     udp echo                 wasi:sockets");
    println!("  set <k> <v>    store a value            wasi:filesystem");
    println!("  get <k>        retrieve a value         wasi:filesystem");
    println!("  del <k>        delete a key             wasi:filesystem");
    println!("  keys           list stored keys         wasi:filesystem");
    println!("  history        command history          wasi:filesystem");
    println!("  flood [bytes]  udp flood stream (^C)    wasi:sockets + clocks");
    println!("  help           this message");
}

fn cmd_time() {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = now.as_secs();

    // manual UTC breakdown
    let days_since_epoch = secs / 86400;
    let time_of_day = secs % 86400;
    let h = time_of_day / 3600;
    let m = (time_of_day % 3600) / 60;
    let s = time_of_day % 60;
    let ns = now.subsec_nanos();

    // date from days (simplified gregorian)
    let (year, month, day) = days_to_ymd(days_since_epoch as i64);

    println!("{year:04}-{month:02}-{day:02}T{h:02}:{m:02}:{s:02}.{ns:09}Z");
    eprintln!("wasi:clocks/wall-clock");
}

fn days_to_ymd(days: i64) -> (i64, u32, u32) {
    // Algorithm from http://howardhinnant.github.io/date_algorithms.html
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

fn cmd_env() {
    let vars: Vec<(String, String)> = std::env::vars().collect();
    if vars.is_empty() {
        println!("(no environment variables set)");
    } else {
        for (k, v) in &vars {
            println!("{k}={v}");
        }
    }
    eprintln!("wasi:cli/environment · {} var(s)", vars.len());
}

fn cmd_rand(n: usize) {
    // Use HashMap's random state which is seeded from wasi:random
    use std::collections::HashMap;
    use std::hash::{BuildHasher, Hasher};

    let mut bytes = Vec::with_capacity(n);
    let mut i = 0u64;
    while bytes.len() < n {
        let map: HashMap<u64, ()> = HashMap::new();
        let mut hasher = map.hasher().build_hasher();
        hasher.write_u64(i);
        let hash = hasher.finish().to_le_bytes();
        bytes.extend_from_slice(&hash[..std::cmp::min(8, n - bytes.len())]);
        i += 1;
    }

    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ");
    println!("{hex}");
    eprintln!("wasi:random · {n} bytes");
}

fn cmd_pi(samples: u64) {
    use std::collections::HashMap;
    use std::hash::{BuildHasher, Hasher};

    let start = Instant::now();
    let mut inside = 0u64;

    for i in 0..samples {
        let map: HashMap<u64, ()> = HashMap::new();
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

    println!("{pi:.8}  (error: {err:.8}, {samples} samples)");
    eprintln!("wasi:clocks/monotonic-clock · {elapsed:?}");
}

fn cmd_fib(n: u64) {
    let start = Instant::now();
    let result = if n <= 186 {
        let mut memo = vec![0u128; (n + 1) as usize];
        if n >= 1 { memo[1] = 1; }
        for i in 2..=n as usize {
            memo[i] = memo[i - 1] + memo[i - 2];
        }
        memo[n as usize]
    } else {
        // For n > 186, u128 overflows. Just show what we can.
        let mut a: u128 = 0;
        let mut b: u128 = 1;
        for _ in 0..n {
            let t = b;
            b = a.wrapping_add(b);
            a = t;
        }
        a
    };
    let elapsed = start.elapsed();
    println!("fib({n}) = {result}");
    eprintln!("wasi:clocks/monotonic-clock · {elapsed:?}");
}

fn cmd_sort(n: usize) {
    let start = Instant::now();

    // Generate pseudo-random data using hash-based randomness
    use std::collections::HashMap;
    use std::hash::{BuildHasher, Hasher};

    let mut data: Vec<u32> = Vec::with_capacity(n);
    for i in 0..n {
        let map: HashMap<u64, ()> = HashMap::new();
        let mut h = map.hasher().build_hasher();
        h.write_usize(i);
        data.push(h.finish() as u32);
    }
    let gen_time = start.elapsed();

    let sort_start = Instant::now();
    data.sort_unstable();
    let sort_time = sort_start.elapsed();

    let total = start.elapsed();
    println!("sorted {n} u32s in {sort_time:?}");
    eprintln!("gen: {gen_time:?} · sort: {sort_time:?} · total: {total:?}");
}

fn cmd_bench() {
    let t = Instant::now();

    // Arithmetic
    let n = 10_000_000u64;
    let arith_start = Instant::now();
    let mut acc: u64 = 0;
    for i in 0..n {
        acc = acc.wrapping_add(i).wrapping_mul(7).wrapping_add(3);
    }
    let arith = arith_start.elapsed();
    let _ = acc; // prevent optimization

    // Sort
    let sort_n = 10_000;
    let sort_start = Instant::now();
    let mut data: Vec<u32> = (0..sort_n).rev().collect();
    data.sort_unstable();
    let sort = sort_start.elapsed();

    // Strings
    let str_start = Instant::now();
    let mut s = String::new();
    for i in 0..1_000 {
        s.push_str(&format!("{i:04x}"));
    }
    let _ = s.len();
    let strings = str_start.elapsed();

    // Fibonacci
    let fib_start = Instant::now();
    let mut a: u128 = 0;
    let mut b: u128 = 1;
    for _ in 0..186 {
        let tmp = b;
        b = a.wrapping_add(b);
        a = tmp;
    }
    let _ = a;
    let fib = fib_start.elapsed();

    let total = t.elapsed();

    println!("arithmetic  {n:>10} ops    {arith:>12?}    {:.1} ns/op", arith.as_nanos() as f64 / n as f64);
    println!("sort        {sort_n:>10} u32s   {sort:>12?}");
    println!("strings     {:>10} concat {strings:>12?}", "1K");
    println!("fibonacci   {:>10}        {fib:>12?}", "fib(186)");
    println!("─────────────────────────────────────────────");
    println!("total                      {total:>12?}");
    eprintln!("wasi:clocks/monotonic-clock");
}

fn log_history(cmd: &str) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open("/state/.history") {
        let _ = writeln!(f, "{cmd}");
    }
}

fn cmd_set(key: &str, value: &str) {
    if key.is_empty() {
        println!("usage: set <key> <value>");
        return;
    }
    match std::fs::write(format!("/state/{key}"), value) {
        Ok(_) => println!("{key} = {value}"),
        Err(e) => println!("error: {e}"),
    }
    eprintln!("wasi:filesystem · write");
}

fn cmd_get(key: &str) {
    if key.is_empty() {
        println!("usage: get <key>");
        return;
    }
    match std::fs::read_to_string(format!("/state/{key}")) {
        Ok(v) => println!("{v}"),
        Err(_) => println!("(not found)"),
    }
    eprintln!("wasi:filesystem · read");
}

fn cmd_del(key: &str) {
    if key.is_empty() {
        println!("usage: del <key>");
        return;
    }
    match std::fs::remove_file(format!("/state/{key}")) {
        Ok(_) => println!("deleted {key}"),
        Err(_) => println!("(not found)"),
    }
    eprintln!("wasi:filesystem · remove");
}

fn cmd_keys() {
    match std::fs::read_dir("/state") {
        Ok(entries) => {
            let mut keys: Vec<String> = entries
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().to_string())
                .filter(|name| !name.starts_with('.'))
                .collect();
            keys.sort();
            if keys.is_empty() {
                println!("(empty)");
            } else {
                for k in &keys {
                    if let Ok(v) = std::fs::read_to_string(format!("/state/{k}")) {
                        println!("  {k} = {v}");
                    }
                }
                eprintln!("wasi:filesystem · {} key(s)", keys.len());
            }
        }
        Err(_) => println!("(empty)"),
    }
}

fn cmd_history() {
    match std::fs::read_to_string("/state/.history") {
        Ok(h) => {
            let lines: Vec<&str> = h.lines().collect();
            if lines.is_empty() {
                println!("(no history)");
            } else {
                for (i, line) in lines.iter().enumerate() {
                    println!("  {:>3}  {line}", i + 1);
                }
                eprintln!("wasi:filesystem · {} entries", lines.len());
            }
        }
        Err(_) => println!("(no history)"),
    }
}

fn cmd_flood(payload_size: usize) {
    let sender = UdpSocket::bind("127.0.0.1:0").expect("bind sender");
    let receiver = UdpSocket::bind("127.0.0.1:0").expect("bind receiver");
    let receiver_addr = receiver.local_addr().unwrap();

    let payload: Vec<u8> = (0..payload_size).map(|i| (i % 256) as u8).collect();
    let mut buf = vec![0u8; payload_size + 64];

    let start = Instant::now();
    let mut seq: u64 = 0;
    let mut bytes_sent: u64 = 0;
    let mut interval_start = Instant::now();

    loop {
        seq += 1;

        sender.send_to(&payload, receiver_addr).unwrap();
        let (len, from) = receiver.recv_from(&mut buf).unwrap();
        receiver.send_to(&buf[..len], from).unwrap();
        let (len, _) = sender.recv_from(&mut buf).unwrap();

        bytes_sent += (len as u64) * 2;

        // Print stats every 500 packets
        if seq % 500 == 0 {
            let elapsed = interval_start.elapsed();
            let total = start.elapsed();
            let throughput = bytes_sent as f64 / total.as_secs_f64() / 1024.0 / 1024.0;
            println!(
                "seq={seq:<8} {len}B rtt={elapsed:>10?}/500pkts  total={total:.1?}  {throughput:.1} MB/s  {bytes_sent} bytes",
            );
            interval_start = Instant::now();
        }
    }
}

fn cmd_echo(msg: &str) {
    if msg.is_empty() {
        println!("usage: echo <message>");
        return;
    }

    let sender = UdpSocket::bind("127.0.0.1:0").expect("bind sender");
    let receiver = UdpSocket::bind("127.0.0.1:0").expect("bind receiver");
    let receiver_addr = receiver.local_addr().unwrap();

    let start = Instant::now();

    sender.send_to(msg.as_bytes(), receiver_addr).unwrap();

    let mut buf = [0u8; 4096];
    let (len, from) = receiver.recv_from(&mut buf).unwrap();
    let got = String::from_utf8_lossy(&buf[..len]).to_string();

    // Echo it back
    receiver.send_to(got.as_bytes(), from).unwrap();
    let (len, _) = sender.recv_from(&mut buf).unwrap();
    let echo = String::from_utf8_lossy(&buf[..len]);

    let rtt = start.elapsed();

    println!("{echo}");
    eprintln!("wasi:sockets/udp · rtt: {rtt:?}");
}
