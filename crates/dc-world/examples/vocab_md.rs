//! Prints the generated tables of docs/MAPKIT_VOCAB.md (`dc_world::vocab::markdown`). Usage:
//! `cargo run -p dc-world --example vocab_md`, then paste the output between the doc's generated-block markers.

fn main() {
    print!("{}", dc_world::vocab::markdown());
}
