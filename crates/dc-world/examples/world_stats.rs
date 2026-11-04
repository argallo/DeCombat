//! Prints per-map counts (FMT_DAT85.md §11.1 / §11.2 columns) for every `.DAT` entry of the user's world
//! archive. Usage: `CA_ROM=<client dir> cargo run -p dc-world --example world_stats [-- --classes]`.
//! Output is computed from the user's files at run time; nothing is stored.

use dc_world::{ObjectRole, World};

fn main() {
    let show_classes = std::env::args().any(|a| a == "--classes");
    let rom = match dc_rez::dc_rom::resolve_rom() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("world_stats: {e}");
            std::process::exit(2);
        }
    };
    let vfs = dc_rez::RezVfs::new(&rom);
    let entries = vfs.list_entries("WORLDS.rez").expect("list world archive");
    for (name, _, _) in entries
        .iter()
        .filter(|e| e.0.to_ascii_uppercase().ends_with(".DAT"))
    {
        // World entries are stored unencrypted; RezVfs::entry returns non-text entries as stored.
        let data = vfs
            .entry("WORLDS.rez", name)
            .expect("read entry")
            .expect("entry present");
        let w = match World::parse(&data) {
            Ok(w) => w,
            Err(e) => {
                println!("{name}\tERROR {e}");
                continue;
            }
        };
        let layout = match w.check_layout() {
            Ok(()) => "tiled".to_string(),
            Err(e) => format!("LAYOUT {e}"),
        };
        let s = w.stats();
        let r = s.render;
        println!(
            "{name}\t{}\tqt={}\tmodels={}({})\tnodes={}\tpolys={}\tpoints={}\tplanes={}\tsurf={}\ttex={}\tobj={}\tprops={}\tclasses={}\tblind={}\tblk={}\tpblk={}\tgrid={:?}\ttop: blocks={} verts={} tris={} sect={} lm={} models={}\tall: blocks={} verts={} tris={} sect={} lm={} models={}\tsky={}\tocc={}\tlgroups={}\tlggrids={}\t{layout}",
            s.bytes, s.quadtree_nodes, s.models, s.dual, s.bsp_nodes, s.bsp_polys, s.points, s.planes, s.surfaces,
            s.textures, s.objects, s.properties, s.classes, s.blind, s.blockers, s.particle_blockers, s.light_grid,
            s.render_top.blocks, s.render_top.vertices, s.render_top.triangles, s.render_top.sections,
            s.render_top.lightmap_bytes, s.render_top.models, r.blocks, r.vertices, r.triangles, r.sections,
            r.lightmap_bytes, r.models, r.sky_portals, r.occluders, r.light_groups, s.lg_grids
        );
        let roles: Vec<String> = ObjectRole::ALL
            .iter()
            .map(|&ro| format!("{:?}={}", ro, w.objects_with_role(ro).count()))
            .collect();
        println!("  roles {}", roles.join(" "));
        let mut joined = Vec::new();
        for c in w.class_names() {
            joined.extend_from_slice(c);
            joined.push(b'\n');
        }
        println!(
            "  class-list sha256 {}",
            dc_rez::dc_rom::sha256_hex(&joined)
        );
        if show_classes {
            for c in w.class_names() {
                println!("    {}", String::from_utf8_lossy(c));
            }
        }
    }
}
