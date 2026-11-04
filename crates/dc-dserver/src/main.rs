//! `dc-dserver`: the DeCombat dedicated game server process (slice 1). Spawned by the lobby with pipes
//! (`DSERVER_PIPE.md`); see the library docs for what it does.

fn main() {
    // the DC_* layer (and DC_CONFIG) applies: the lobby's environment, e.g. the launcher's
    // DC_ROOM__BLAST_DAMAGE_SCALE, reaches the spawned dserver (DSV-168; plain `parse` dropped it)
    let env: Vec<(String, String)> = std::env::vars().collect();
    match dc_dserver::cli::parse_with_env(std::env::args().skip(1), &env) {
        Ok(args) => std::process::exit(dc_dserver::runner::run(args)),
        Err(e) => {
            println!("dc-dserver: error: {e}");
            println!("{}", dc_dserver::cli::USAGE);
            println!("{}", dc_dserver::pipe::END_LINE);
            std::process::exit(64);
        }
    }
}
