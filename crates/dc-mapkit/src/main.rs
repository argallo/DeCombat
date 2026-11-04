//! `dc-mapkit` binary: the verbs of [`dc_mapkit::cli`]. Stdout carries only the verb's output; logging goes to
//! stderr (the `mcp` verb relies on that).

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let stdout = std::io::stdout();
    let stderr = std::io::stderr();
    let code = dc_mapkit::cli::run_cli(&args, &mut stdout.lock(), &mut stderr.lock());
    if code != 0 {
        std::process::exit(code);
    }
}
