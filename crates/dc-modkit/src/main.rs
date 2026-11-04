use std::env;
use std::process;

fn main() {
    let args: Vec<String> = env::args().collect();
    let code = dc_modkit::run_cli(&args);
    if code != 0 {
        process::exit(code);
    }
}
