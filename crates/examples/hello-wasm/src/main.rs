fn main() {
    println!("Hello from WebAssembly (WASI P2)!");

    let numbers: Vec<i32> = (1..=10).collect();
    let sum: i32 = numbers.iter().sum();
    println!("Sum of 1..10 = {sum}");

    let args: Vec<String> = std::env::args().collect();
    println!("Program name: {}", args[0]);
    if args.len() > 1 {
        println!("Arguments: {}", args[1..].join(", "));
    }
}
