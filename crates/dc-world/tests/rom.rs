//! ROM-gated validation against FMT_DAT85.md section 11 (skipped when CA_ROM is unset). Maps are keyed by
//! file size; no ROM strings appear here, class lists are compared by SHA-256.

use dc_world::{FieldValue, ObjectRole, PropKey, PropKind, World};

struct MapExpect {
    bytes: u64,
    qt: u64,
    models: u64,
    dual: u64,
    bsp_nodes: u64,
    polys: u64,
    points: u64,
    planes: u64,
    surfaces: u64,
    textures: u64,
    objects: u64,
    classes: u64,
    blind: u64,
    blockers: u64,
    pblockers: u64,
    grid: [u32; 3],
    /// Top-level render world only (OBSERVED: the spec's block/vertex/triangle columns count level 0).
    rblocks: u64,
    rverts: u64,
    rtris: u64,
    /// All render levels (OBSERVED: the spec's section and lightmap columns include nested models).
    sections: u64,
    lightmap: u64,
    rmodels: u64,
    lg_grids: u64,
    /// player spawn, item spawn, kill volume, trigger, mode controller, flag, flag zone, bomb zone
    roles: [u64; 8],
    /// sha256 of the sorted distinct class names, each followed by a newline (OBSERVED from the ROM).
    class_sha256: &'static str,
}

// SPEC: FMT_DAT85.md §11.1 and §11.2 (rows in the spec's order, keyed by byte size)
const MAPS: [MapExpect; 21] = [
    MapExpect {
        bytes: 7717853,
        qt: 341,
        models: 56,
        dual: 55,
        bsp_nodes: 64977,
        polys: 22615,
        points: 15995,
        planes: 22639,
        surfaces: 157,
        textures: 186,
        objects: 311,
        classes: 25,
        blind: 33,
        blockers: 35,
        pblockers: 15,
        grid: [32, 99, 28],
        rblocks: 37,
        rverts: 56948,
        rtris: 21159,
        sections: 563,
        lightmap: 153225,
        rmodels: 55,
        lg_grids: 0,
        roles: [80, 8, 23, 25, 4, 2, 2, 0],
        class_sha256: "37b827ce9350eadc23fbedbb1dc04a2ddc69b42aefb0a6b9054081935e0c44b3",
    },
    MapExpect {
        bytes: 11197353,
        qt: 5461,
        models: 38,
        dual: 37,
        bsp_nodes: 58144,
        polys: 32029,
        points: 22303,
        planes: 26636,
        surfaces: 241,
        textures: 240,
        objects: 399,
        classes: 32,
        blind: 7,
        blockers: 126,
        pblockers: 8,
        grid: [82, 82, 82],
        rblocks: 71,
        rverts: 91593,
        rtris: 39188,
        sections: 1023,
        lightmap: 329713,
        rmodels: 37,
        lg_grids: 2,
        roles: [8, 2, 8, 26, 1, 0, 0, 0],
        class_sha256: "3a53b66384914b0344650c2cf3f52d4331fcb61da6542ab268f267cb3db521b3",
    },
    MapExpect {
        bytes: 18108691,
        qt: 1365,
        models: 80,
        dual: 79,
        bsp_nodes: 82219,
        polys: 39263,
        points: 42587,
        planes: 37954,
        surfaces: 306,
        textures: 321,
        objects: 298,
        classes: 21,
        blind: 0,
        blockers: 101,
        pblockers: 44,
        grid: [61, 159, 61],
        rblocks: 111,
        rverts: 156319,
        rtris: 74422,
        sections: 1990,
        lightmap: 2185998,
        rmodels: 79,
        lg_grids: 0,
        roles: [80, 12, 55, 17, 4, 2, 2, 0],
        class_sha256: "e473d4b505ca39aa227265940c2b2561be429fefcd98e87f64748b5d3160c7c5",
    },
    MapExpect {
        bytes: 16008889,
        qt: 341,
        models: 42,
        dual: 41,
        bsp_nodes: 105201,
        polys: 35267,
        points: 26180,
        planes: 28499,
        surfaces: 125,
        textures: 160,
        objects: 228,
        classes: 21,
        blind: 0,
        blockers: 192,
        pblockers: 0,
        grid: [37, 59, 37],
        rblocks: 95,
        rverts: 133373,
        rtris: 57603,
        sections: 899,
        lightmap: 2111383,
        rmodels: 41,
        lg_grids: 0,
        roles: [84, 11, 10, 19, 4, 2, 2, 0],
        class_sha256: "7d04e8200edec44548e3c254a0066181bb3732162484e0d21bcab05e2b4d4684",
    },
    MapExpect {
        bytes: 24496461,
        qt: 5461,
        models: 96,
        dual: 95,
        bsp_nodes: 117406,
        polys: 49182,
        points: 40982,
        planes: 40759,
        surfaces: 334,
        textures: 409,
        objects: 408,
        classes: 25,
        blind: 4,
        blockers: 149,
        pblockers: 16,
        grid: [82, 82, 82],
        rblocks: 185,
        rverts: 213803,
        rtris: 93982,
        sections: 2429,
        lightmap: 3640391,
        rmodels: 95,
        lg_grids: 0,
        roles: [98, 16, 61, 19, 5, 2, 2, 0],
        class_sha256: "e12e0adbef181978b93c290a488006bfbb671694e7369180940f7cc15b2b82f6",
    },
    MapExpect {
        bytes: 23283094,
        qt: 1365,
        models: 87,
        dual: 86,
        bsp_nodes: 84095,
        polys: 38757,
        points: 50137,
        planes: 35069,
        surfaces: 472,
        textures: 466,
        objects: 392,
        classes: 25,
        blind: 0,
        blockers: 146,
        pblockers: 0,
        grid: [102, 160, 75],
        rblocks: 133,
        rverts: 186934,
        rtris: 92810,
        sections: 2829,
        lightmap: 4493871,
        rmodels: 86,
        lg_grids: 0,
        roles: [104, 17, 52, 22, 5, 2, 2, 2],
        class_sha256: "13d80bbc40c6516f93928c0d26d1c565d867dc07843e8939de7b51e080bdbde5",
    },
    MapExpect {
        bytes: 15673717,
        qt: 21845,
        models: 63,
        dual: 62,
        bsp_nodes: 87759,
        polys: 32586,
        points: 33747,
        planes: 31843,
        surfaces: 317,
        textures: 279,
        objects: 317,
        classes: 22,
        blind: 0,
        blockers: 42,
        pblockers: 0,
        grid: [263, 525, 263],
        rblocks: 93,
        rverts: 113675,
        rtris: 52768,
        sections: 1276,
        lightmap: 1271851,
        rmodels: 62,
        lg_grids: 0,
        roles: [100, 11, 38, 25, 5, 2, 2, 0],
        class_sha256: "d676fe80c5787405a5146dffb4cd09ce89756ffecaec7d652445d17617abd09c",
    },
    MapExpect {
        bytes: 23852033,
        qt: 1365,
        models: 212,
        dual: 211,
        bsp_nodes: 110370,
        polys: 42045,
        points: 37974,
        planes: 35124,
        surfaces: 487,
        textures: 667,
        objects: 586,
        classes: 24,
        blind: 0,
        blockers: 38,
        pblockers: 0,
        grid: [64, 58, 59],
        rblocks: 129,
        rverts: 185471,
        rtris: 89149,
        sections: 2124,
        lightmap: 4545822,
        rmodels: 211,
        lg_grids: 2,
        roles: [60, 0, 88, 48, 3, 0, 0, 0],
        class_sha256: "e95139247a2f22d2bf237afd21d15849bcebf10bad3fe645206c98627090c12f",
    },
    MapExpect {
        bytes: 19958493,
        qt: 341,
        models: 38,
        dual: 37,
        bsp_nodes: 86830,
        polys: 36174,
        points: 37501,
        planes: 34437,
        surfaces: 206,
        textures: 208,
        objects: 260,
        classes: 20,
        blind: 0,
        blockers: 81,
        pblockers: 0,
        grid: [32, 160, 36],
        rblocks: 113,
        rverts: 172515,
        rtris: 81002,
        sections: 1759,
        lightmap: 3154489,
        rmodels: 37,
        lg_grids: 0,
        roles: [80, 12, 20, 17, 4, 2, 2, 0],
        class_sha256: "b4443d524f925b71b0b4ca22fae5eccc070d43bbbe38f2d0beae52a423d30b6e",
    },
    MapExpect {
        bytes: 26185760,
        qt: 1365,
        models: 141,
        dual: 140,
        bsp_nodes: 140995,
        polys: 49830,
        points: 58269,
        planes: 41916,
        surfaces: 674,
        textures: 666,
        objects: 552,
        classes: 25,
        blind: 10,
        blockers: 161,
        pblockers: 0,
        grid: [70, 73, 73],
        rblocks: 169,
        rverts: 234775,
        rtris: 121237,
        sections: 3264,
        lightmap: 2279202,
        rmodels: 140,
        lg_grids: 0,
        roles: [106, 19, 79, 21, 5, 2, 2, 2],
        class_sha256: "dce4dde4d0fb41ad49cc9c5cc9914eff7c4e963110fb279b7bce66395d7073bb",
    },
    MapExpect {
        bytes: 24928939,
        qt: 1365,
        models: 69,
        dual: 68,
        bsp_nodes: 94261,
        polys: 50406,
        points: 48509,
        planes: 46947,
        surfaces: 298,
        textures: 322,
        objects: 448,
        classes: 25,
        blind: 10,
        blockers: 150,
        pblockers: 0,
        grid: [40, 41, 45],
        rblocks: 137,
        rverts: 211577,
        rtris: 98859,
        sections: 1956,
        lightmap: 4485917,
        rmodels: 68,
        lg_grids: 0,
        roles: [88, 0, 34, 20, 4, 2, 2, 2],
        class_sha256: "c28ae0e6adf72f7e3129282fac2eff9153fbf9add655b82b0136ad44e1cf3261",
    },
    MapExpect {
        bytes: 26467089,
        qt: 21845,
        models: 99,
        dual: 98,
        bsp_nodes: 182499,
        polys: 72146,
        points: 50365,
        planes: 70711,
        surfaces: 211,
        textures: 219,
        objects: 295,
        classes: 19,
        blind: 13,
        blockers: 60,
        pblockers: 0,
        grid: [189, 91, 181],
        rblocks: 187,
        rverts: 242337,
        rtris: 93374,
        sections: 1381,
        lightmap: 1348865,
        rmodels: 98,
        lg_grids: 0,
        roles: [48, 0, 73, 13, 3, 2, 2, 0],
        class_sha256: "4818b7036762f835b610282a826e12765aabfb8306465389a2a2f87022b623d2",
    },
    MapExpect {
        bytes: 33165709,
        qt: 341,
        models: 112,
        dual: 111,
        bsp_nodes: 147670,
        polys: 68160,
        points: 63784,
        planes: 63949,
        surfaces: 460,
        textures: 392,
        objects: 357,
        classes: 24,
        blind: 7,
        blockers: 390,
        pblockers: 0,
        grid: [33, 66, 31],
        rblocks: 183,
        rverts: 306765,
        rtris: 139265,
        sections: 3656,
        lightmap: 3501704,
        rmodels: 111,
        lg_grids: 0,
        roles: [104, 15, 67, 22, 5, 2, 2, 2],
        class_sha256: "ccda191a719cdca8855e9cd0e5444226e91ca879328d059faf3a58f4c49f0abe",
    },
    MapExpect {
        bytes: 54334765,
        qt: 1365,
        models: 118,
        dual: 117,
        bsp_nodes: 245759,
        polys: 113429,
        points: 101287,
        planes: 105860,
        surfaces: 503,
        textures: 504,
        objects: 3017,
        classes: 38,
        blind: 1,
        blockers: 459,
        pblockers: 0,
        grid: [52, 66, 66],
        rblocks: 301,
        rverts: 429998,
        rtris: 189786,
        sections: 5491,
        lightmap: 4277569,
        rmodels: 117,
        lg_grids: 0,
        roles: [32, 0, 20, 151, 1, 0, 0, 0],
        class_sha256: "565aabef275bf2a790395f024053a51a7a4163acafad37bcd4eabe9cbf3ca407",
    },
    MapExpect {
        bytes: 20849375,
        qt: 1309,
        models: 94,
        dual: 93,
        bsp_nodes: 105729,
        polys: 49769,
        points: 54477,
        planes: 48572,
        surfaces: 368,
        textures: 422,
        objects: 559,
        classes: 25,
        blind: 0,
        blockers: 171,
        pblockers: 0,
        grid: [70, 132, 78],
        rblocks: 131,
        rverts: 192985,
        rtris: 86636,
        sections: 2006,
        lightmap: 853756,
        rmodels: 93,
        lg_grids: 0,
        roles: [104, 16, 50, 23, 5, 2, 2, 2],
        class_sha256: "dae2a70e4a4bef94e39f8f07eb29f3f925f0df8699b0a354bd1a5a9b70a52174",
    },
    MapExpect {
        bytes: 26424556,
        qt: 21845,
        models: 118,
        dual: 117,
        bsp_nodes: 83124,
        polys: 40182,
        points: 44317,
        planes: 34980,
        surfaces: 308,
        textures: 375,
        objects: 404,
        classes: 23,
        blind: 0,
        blockers: 646,
        pblockers: 0,
        grid: [213, 132, 197],
        rblocks: 137,
        rverts: 203087,
        rtris: 100283,
        sections: 2172,
        lightmap: 5710021,
        rmodels: 117,
        lg_grids: 0,
        roles: [100, 12, 65, 25, 5, 2, 2, 0],
        class_sha256: "937d4b31a9a2282b1905c5e19f56866a569c0e455f7a9dcf73894ae78654f8ad",
    },
    MapExpect {
        bytes: 24792990,
        qt: 21725,
        models: 115,
        dual: 114,
        bsp_nodes: 74497,
        polys: 37710,
        points: 42228,
        planes: 34297,
        surfaces: 291,
        textures: 360,
        objects: 273,
        classes: 18,
        blind: 0,
        blockers: 624,
        pblockers: 0,
        grid: [184, 160, 158],
        rblocks: 161,
        rverts: 199323,
        rtris: 98499,
        sections: 2295,
        lightmap: 5145460,
        rmodels: 114,
        lg_grids: 0,
        roles: [20, 0, 75, 9, 1, 0, 0, 0],
        class_sha256: "4fb75c1f91a416bcd8a304ecf016a989c1abc9127454fc47e21449818346ba93",
    },
    MapExpect {
        bytes: 18095257,
        qt: 5461,
        models: 110,
        dual: 109,
        bsp_nodes: 91743,
        polys: 38402,
        points: 31128,
        planes: 34339,
        surfaces: 414,
        textures: 494,
        objects: 310,
        classes: 16,
        blind: 6,
        blockers: 50,
        pblockers: 0,
        grid: [80, 81, 81],
        rblocks: 145,
        rverts: 148383,
        rtris: 67980,
        sections: 2115,
        lightmap: 3212853,
        rmodels: 109,
        lg_grids: 0,
        roles: [40, 0, 55, 11, 2, 0, 0, 0],
        class_sha256: "112a8693fd065b7ed730084fef9826a2f4972ee6ce97add8baf7bfff29e70d67",
    },
    MapExpect {
        bytes: 19640583,
        qt: 5461,
        models: 103,
        dual: 102,
        bsp_nodes: 96701,
        polys: 39703,
        points: 31932,
        planes: 35665,
        surfaces: 415,
        textures: 482,
        objects: 269,
        classes: 16,
        blind: 6,
        blockers: 45,
        pblockers: 0,
        grid: [80, 81, 81],
        rblocks: 161,
        rverts: 171864,
        rtris: 76012,
        sections: 2239,
        lightmap: 2937330,
        rmodels: 102,
        lg_grids: 0,
        roles: [20, 0, 56, 9, 1, 0, 0, 0],
        class_sha256: "112a8693fd065b7ed730084fef9826a2f4972ee6ce97add8baf7bfff29e70d67",
    },
    MapExpect {
        bytes: 18986126,
        qt: 1081,
        models: 102,
        dual: 101,
        bsp_nodes: 107942,
        polys: 30489,
        points: 39272,
        planes: 27524,
        surfaces: 436,
        textures: 463,
        objects: 444,
        classes: 27,
        blind: 9,
        blockers: 136,
        pblockers: 0,
        grid: [70, 164, 57],
        rblocks: 127,
        rverts: 147735,
        rtris: 75610,
        sections: 1975,
        lightmap: 3147722,
        rmodels: 101,
        lg_grids: 0,
        roles: [124, 11, 54, 30, 6, 2, 2, 2],
        class_sha256: "c05a7cc86dfa2a502f5c81753454ace96ab9b2e7049383ec5c942fc7f75c5937",
    },
    MapExpect {
        bytes: 31297063,
        qt: 5461,
        models: 41,
        dual: 40,
        bsp_nodes: 136877,
        polys: 46368,
        points: 56456,
        planes: 40348,
        surfaces: 293,
        textures: 263,
        objects: 245,
        classes: 22,
        blind: 6,
        blockers: 187,
        pblockers: 0,
        grid: [150, 90, 153],
        rblocks: 221,
        rverts: 274681,
        rtris: 139937,
        sections: 3363,
        lightmap: 5028718,
        rmodels: 40,
        lg_grids: 0,
        roles: [80, 15, 25, 17, 4, 2, 2, 0],
        class_sha256: "20ff467f1a7f20566c522c5eea9557f681aebc28900a5ba916fc3ffff50da57b",
    },
];

const ROLE_COLUMNS: [ObjectRole; 8] = [
    ObjectRole::PlayerSpawn,
    ObjectRole::ItemSpawn,
    ObjectRole::KillVolume,
    ObjectRole::TriggerVolume,
    ObjectRole::ModeController,
    ObjectRole::CaptureFlag,
    ObjectRole::FlagZone,
    ObjectRole::BombSite,
];

// SPEC: FMT_DAT85.md §11.1 (occluders and render light groups per map, keyed here by byte size)
fn occluders(bytes: u64) -> u64 {
    match bytes {
        24496461 => 6,
        23852033 => 7,
        33165709 => 15,
        54334765 => 38,
        31297063 => 13,
        _ => 0,
    }
}

fn light_groups(bytes: u64) -> u64 {
    match bytes {
        11197353 => 79,
        23852033 => 55,
        _ => 0,
    }
}

/// All `.DAT` entries of the world archive. World entries are stored unencrypted and `RezVfs::entry`
/// returns non-text entries as stored.
fn world_files(rom: &std::path::Path) -> Vec<(String, Vec<u8>)> {
    let vfs = dc_rez::RezVfs::new(rom);
    let mut out = Vec::new();
    for (name, _, _) in vfs.list_entries("WORLDS.rez").expect("list") {
        if !name.to_ascii_uppercase().ends_with(".DAT") {
            continue;
        }
        let data = vfs
            .entry("WORLDS.rez", &name)
            .expect("entry")
            .expect("present");
        assert!(
            data.starts_with(&dc_world::WORLD_VERSION.to_le_bytes()),
            "{name}: stored bytes carry the version word"
        );
        out.push((name, data));
    }
    out
}

#[test]
fn rom_worlds_match_spec_validation() {
    let rom = dc_rez::dc_rom::rom_or_skip!();
    let files = world_files(&rom);
    assert_eq!(files.len(), 21, "world archive .DAT entries");
    let mut seen = Vec::new();
    let (mut tot_obj, mut tot_props, mut tot_models, mut tot_dual) = (0u64, 0u64, 0u64, 0u64);
    for (idx, (_, data)) in files.iter().enumerate() {
        let w = World::parse(data).unwrap_or_else(|e| panic!("world #{idx}: {e}"));
        // SPEC: FMT_DAT85.md §3 / §11: sections tile the file and the render data ends at EOF
        w.check_layout()
            .unwrap_or_else(|e| panic!("world #{idx}: {e}"));
        assert_eq!(w.spans.len(), 7);
        assert_eq!(w.spans.last().unwrap().end, data.len());
        assert_eq!(w.spans[0].end, w.header.objects_at as usize);
        assert_eq!(w.header.reserved, [0; 8]);
        let s = w.stats();
        let e = MAPS
            .iter()
            .find(|m| m.bytes == s.bytes)
            .unwrap_or_else(|| panic!("world #{idx}: size not in table"));
        assert!(!seen.contains(&e.bytes));
        seen.push(e.bytes);
        let got = (
            s.quadtree_nodes,
            s.models,
            s.dual,
            s.bsp_nodes,
            s.bsp_polys,
            s.points,
            s.planes,
            s.surfaces,
            s.textures,
        );
        let want = (
            e.qt,
            e.models,
            e.dual,
            e.bsp_nodes,
            e.polys,
            e.points,
            e.planes,
            e.surfaces,
            e.textures,
        );
        assert_eq!(got, want, "bsp counts, world of {} bytes", e.bytes);
        let got = (
            s.objects,
            s.classes,
            s.blind,
            s.blockers,
            s.particle_blockers,
            s.light_grid,
            s.lg_grids,
        );
        let want = (
            e.objects,
            e.classes,
            e.blind,
            e.blockers,
            e.pblockers,
            e.grid,
            e.lg_grids,
        );
        assert_eq!(got, want, "object/side counts, world of {} bytes", e.bytes);
        let got = (
            s.render_top.blocks,
            s.render_top.vertices,
            s.render_top.triangles,
            s.render.sections,
            s.render.lightmap_bytes,
            s.render_top.models,
        );
        let want = (
            e.rblocks, e.rverts, e.rtris, e.sections, e.lightmap, e.rmodels,
        );
        assert_eq!(got, want, "render counts, world of {} bytes", e.bytes);
        assert_eq!(s.render.sky_portals, 6);
        assert_eq!(s.render.occluders, occluders(e.bytes));
        assert_eq!(s.render.light_groups, light_groups(e.bytes));
        // SPEC: FMT_DAT85.md §10.1: top-level render models equal the dual BSP models
        assert_eq!(s.render_top.models, s.dual);
        let roles: Vec<u64> = ROLE_COLUMNS
            .iter()
            .map(|&r| w.objects_with_role(r).count() as u64)
            .collect();
        assert_eq!(roles, e.roles, "role counts, world of {} bytes", e.bytes);
        let mut joined = Vec::new();
        for c in w.class_names() {
            joined.extend_from_slice(c);
            joined.push(b'\n');
        }
        assert_eq!(
            dc_rez::dc_rom::sha256_hex(&joined),
            e.class_sha256,
            "class list, world of {} bytes",
            e.bytes
        );
        check_invariants(&w);
        tot_obj += s.objects;
        tot_props += s.properties;
        tot_models += s.models;
        tot_dual += s.dual;
    }
    // SPEC: FMT_DAT85.md §11.1 totals
    assert_eq!(
        (tot_obj, tot_props, tot_models, tot_dual),
        (10372, 276740, 1934, 1913)
    );
}

/// The OBSERVED invariants of FMT_DAT85.md sections 4-9 and 11.
fn check_invariants(w: &World<'_>) {
    assert_eq!(w.quadtree.min, w.extent_min);
    assert_eq!(w.quadtree.max, w.extent_max);
    assert_eq!(w.quadtree.reserved, 0);
    for m in &w.models {
        assert_eq!((m.reserved, m.reserved_a, m.reserved_b), (0, 0, 0));
        assert!(m.leaves.is_empty());
        let sum: u32 = m.polygons.iter().map(|p| p.corner_count as u32).sum();
        assert_eq!(sum, m.declared_poly_verts);
        assert!(m
            .surfaces
            .iter()
            .all(|s| (s.texture as usize) < m.texture_names.len()));
    }
    assert_eq!(w.models.iter().filter(|m| !m.is_dual()).count(), 1);
    assert_eq!(
        w.light_grid.packed.len(),
        w.light_grid.declared_packed_len as usize
    );
    assert_eq!(
        w.light_grid.expand().len() as u64,
        w.light_grid.sample_count() * 3
    );
    assert_eq!((w.blockers.tail, w.particle_blockers.tail), (0, 0));
    for o in &w.objects {
        // SPEC: FMT_DAT85.md §6.1 fixed leading properties
        let p = &o.props;
        assert!(p.len() >= 3);
        assert_eq!((p[0].kind, p[0].flags), (PropKind::Text, 0));
        assert_eq!((p[1].kind, p[1].flags), (PropKind::Vec3, 0x0010_0000));
        assert_eq!((p[2].kind, p[2].flags), (PropKind::Euler, 0));
        assert!(std::ptr::eq(o.prop_by_key(PropKey::Name).unwrap(), &p[0]));
        assert!(std::ptr::eq(
            o.prop_by_key(PropKey::Position).unwrap(),
            &p[1]
        ));
        assert!(std::ptr::eq(
            o.prop_by_key(PropKey::Rotation).unwrap(),
            &p[2]
        ));
        assert!(matches!(p[2].value, FieldValue::Euler([_, _, _, f]) if f == 0.0));
        assert!(o.position().is_some() && o.rotation().is_some() && o.name().is_some());
        for q in p {
            // SPEC: FMT_DAT85.md §6: code 4 absent; payload sizes as tabled
            assert_ne!(q.kind, PropKind::Bits);
            let want = match q.kind {
                PropKind::Text => q.raw.len() + 2,
                PropKind::Vec3 | PropKind::Rgb => 12,
                PropKind::Real | PropKind::Int => 4,
                PropKind::Bool => 1,
                PropKind::Euler => 16,
                _ => panic!("unexpected property code"),
            };
            assert_eq!(q.declared_len as usize, want);
        }
        // SPEC: FMT_DAT85.md §6.4: brush roles link by name to a BSP model; point and box roles never do
        match o.role {
            Some(ObjectRole::KillVolume | ObjectRole::Breakable | ObjectRole::Ladder) => {
                assert!(w.brush_of(o).is_some())
            }
            Some(ObjectRole::PlayerSpawn | ObjectRole::TriggerVolume | ObjectRole::FlagZone) => {
                assert!(w.brush_of(o).is_none())
            }
            _ => {}
        }
        if o.role == Some(ObjectRole::KillVolume) {
            assert!(o.real(PropKey::Damage).is_some());
            assert!(o.text(PropKey::DamageKind).is_some());
        }
        if matches!(
            o.role,
            Some(ObjectRole::TriggerVolume | ObjectRole::FlagZone)
        ) {
            assert!(o.vec3(PropKey::BoxExtent).is_some());
        }
        if o.role == Some(ObjectRole::PlayerSpawn) {
            assert!(o.text(PropKey::ModeFilter).is_some() && o.text(PropKey::TeamFilter).is_some());
        }
    }
    let r = w.render.as_ref().unwrap();
    for g in &r.grids {
        assert_eq!(
            g.data.len() as u64,
            g.dims.iter().map(|&d| d as u64).product::<u64>()
        );
    }
}

#[test]
fn rom_server_parse_skips_render() {
    let rom = dc_rez::dc_rom::rom_or_skip!();
    let (_, data) = world_files(&rom).into_iter().next().expect("one world");
    let w = World::parse_with(&data, dc_world::ParseOptions { render: false }).unwrap();
    assert!(w.render.is_none());
    w.check_layout().unwrap();
    assert_eq!(w.spans.last().unwrap().end, w.header.render_at as usize);
}
