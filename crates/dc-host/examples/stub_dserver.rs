//! A stand-in `dc-dserver` for tests/agent.rs (not a game server): prints `@ready port=<p> world=<w>`, then
//! `@args <argv after the program, joined by '|'>` and `@env rom=<CA_ROM> cwd=<working directory>`, then echoes every
//! stdin line as `echo:<line>`; a `quit` line ends it with `@end` and code 0, stdin EOF with code 0. With `-ignoreeof`
//! it keeps running after stdin EOF (the agent's grace kill ends it). `-exit <n>` exits with code n at once after
//! `@ready`.
// Provenance: ORIGINAL (test helper)

use std::io::{BufRead, Write};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let val = |k: &str| {
        args.iter()
            .position(|a| a == k)
            .and_then(|i| args.get(i + 1))
            .cloned()
            .unwrap_or_default()
    };
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "@ready port={} world={}", val("-port"), val("-world"));
    let _ = writeln!(out, "@args {}", args.join("|"));
    let _ = writeln!(
        out,
        "@env rom={} cwd={}",
        std::env::var("CA_ROM").unwrap_or_default(),
        std::env::current_dir()
            .map(|d| d.display().to_string())
            .unwrap_or_default()
    );
    let _ = out.flush();
    if let Ok(code) = val("-exit").parse::<i32>() {
        std::process::exit(code);
    }
    for line in std::io::stdin().lock().lines() {
        let Ok(l) = line else { break };
        if l == "quit" {
            let _ = writeln!(out, "@end");
            let _ = out.flush();
            return;
        }
        let _ = writeln!(out, "echo:{l}");
        let _ = out.flush();
    }
    if args.iter().any(|a| a == "-ignoreeof") {
        loop {
            std::thread::sleep(std::time::Duration::from_secs(60));
        }
    }
}
