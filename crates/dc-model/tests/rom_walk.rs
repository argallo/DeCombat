//! ROM-gated: walks every `.ltb` entry of every `.rez` archive of the user's client (read-only) and checks the
//! validation facts of FMT_LTB.md §12 plus the observed counts of §5-§11. Counts only; no ROM data is printed
//! or stored. Skips when CA_ROM is unset.

use dc_model::*;
use std::collections::BTreeMap;

#[test]
fn rom_every_ltb() {
    let rom = dc_rom::rom_or_skip!();
    let game = rom.join("Game");
    let mut rez_names: Vec<String> = std::fs::read_dir(&game)
        .expect("Game dir")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.to_ascii_lowercase().ends_with(".rez"))
        .collect();
    rez_names.sort();

    let mut entries = 0usize;
    let mut parsed = 0usize;
    let mut trailing_nonzero = 0usize;
    let mut reencode_diff = 0usize;
    let mut revs: BTreeMap<u32, usize> = BTreeMap::new();
    let mut not_model: BTreeMap<u8, usize> = BTreeMap::new();
    let mut refused: BTreeMap<u32, usize> = BTreeMap::new();
    let mut other_err: Vec<String> = Vec::new();
    let (mut nodes, mut nodes_affine) = (0usize, 0usize);
    let (mut skinned, mut skinned_contig) = (0usize, 0usize);
    let mut kinds: BTreeMap<u32, usize> = BTreeMap::new();
    let mut bodies_exact = 0usize;
    let mut lod_counts: BTreeMap<usize, usize> = BTreeMap::new();
    let mut codecs: BTreeMap<u32, usize> = BTreeMap::new();
    let mut child_slots: BTreeMap<usize, usize> = BTreeMap::new();
    let (mut morph_tracks, mut sockets, mut blend_sets) = (0usize, 0usize, 0usize);
    let (mut hint_clips_eq, mut hint_nodes_eq) = (0usize, 0usize);

    for rez in &rez_names {
        // Model entries are stored unencrypted; RezVfs::entry returns non-text entries as stored.
        let vfs = dc_rez::RezVfs::new(&rom);
        let listing = vfs.list_entries(rez).expect("list rez");
        for (name, _, _) in listing
            .iter()
            .filter(|(n, _, _)| n.to_ascii_lowercase().ends_with(".ltb"))
        {
            entries += 1;
            let bytes = vfs
                .entry(rez, name)
                .expect("read entry")
                .expect("entry present");
            match Model::parse(&bytes) {
                Ok(m) => {
                    parsed += 1;
                    *revs.entry(m.header.model_rev).or_default() += 1;
                    if !m.trailing.is_empty() {
                        trailing_nonzero += 1;
                    }
                    if m.to_bytes().ok().as_deref() != Some(&bytes[..]) {
                        reencode_diff += 1;
                    }
                    nodes += m.nodes.len();
                    nodes_affine += m.nodes.iter().filter(|n| n.last_row_affine()).count();
                    for p in &m.pieces {
                        *lod_counts.entry(p.lods.len()).or_default() += 1;
                    }
                    for l in m.lods() {
                        *kinds.entry(l.mesh.kind()).or_default() += 1;
                        if l.declared_body_len as u64 == l.mesh.body_len() {
                            bodies_exact += 1;
                        }
                        if let Mesh::Skinned(s) = &l.mesh {
                            skinned += 1;
                            if s.spans_contiguous() == Some(true) {
                                skinned_contig += 1;
                            }
                        }
                    }
                    for c in &m.clips {
                        *codecs.entry(c.codec).or_default() += 1;
                        morph_tracks += c
                            .tracks
                            .iter()
                            .filter(|t| matches!(t, Track::Morph { .. }))
                            .count();
                    }
                    *child_slots.entry(m.child_models.len() + 1).or_default() += 1;
                    sockets += m.sockets.len();
                    blend_sets += m.blend_sets.len();
                    hint_clips_eq += (m.hints.clips() as usize == m.clips.len()) as usize;
                    hint_nodes_eq += (m.hints.nodes() as usize == m.nodes.len()) as usize;
                }
                Err(ModelError::NotAModel { file_kind }) => {
                    *not_model.entry(file_kind).or_default() += 1
                }
                Err(ModelError::UnsupportedRevision { rev }) => {
                    *refused.entry(rev).or_default() += 1
                }
                Err(e) => other_err.push(e.to_string()),
            }
        }
    }

    println!(
        "rez={} ltb={entries} parsed={parsed} revs={:?} not_model={:?} refused={:?} other_err={}",
        rez_names.len(),
        revs,
        not_model,
        refused,
        other_err.len()
    );
    println!(
        "nodes={nodes} affine={nodes_affine} kinds={:?} bodies_exact={bodies_exact} skinned={skinned} contiguous={skinned_contig}",
        kinds
    );
    println!(
        "lods={:?} codecs={:?} child_slots={:?} morph_tracks={morph_tracks} sockets={sockets} blend_sets={blend_sets} reencode_diff={reencode_diff}",
        lod_counts,
        codecs,
        child_slots
    );

    // SPEC: FMT_LTB.md §12
    assert_eq!(rez_names.len(), 95);
    assert_eq!(entries, 1610);
    assert!(other_err.is_empty(), "unexpected errors: {other_err:?}");
    assert_eq!(parsed, 1542);
    assert_eq!(trailing_nonzero, 0);
    assert_eq!(revs, BTreeMap::from([(23, 177), (25, 1365)]));
    assert_eq!(not_model, BTreeMap::from([(5, 61), (9, 6)]));
    assert_eq!(refused, BTreeMap::from([(18, 1)]));
    assert_eq!(bodies_exact, 5434);
    // SPEC: FMT_LTB.md §7 (last row (0,0,0,1) in all 48,730 nodes)
    assert_eq!(nodes, 48730);
    assert_eq!(nodes_affine, nodes);
    // SPEC: FMT_LTB.md §6.3 (spans contiguous over all 2,997 skinned bodies)
    assert_eq!(skinned, 2997);
    assert_eq!(skinned_contig, skinned);
    // SPEC: FMT_LTB.md §5 histograms
    assert_eq!(kinds, BTreeMap::from([(4, 2401), (5, 2997), (6, 36)]));
    assert_eq!(lod_counts, BTreeMap::from([(1, 5107), (3, 109)]));
    // SPEC: FMT_LTB.md §10.3, §9, §11.1, §8, §3
    assert_eq!(
        codecs,
        BTreeMap::from([(0, 6389), (1, 1), (2, 118), (3, 2)])
    );
    assert_eq!(morph_tracks, 36);
    assert_eq!(child_slots, BTreeMap::from([(1, 935), (2, 607)]));
    assert_eq!(sockets, 2005);
    assert_eq!(blend_sets, 350);
    assert_eq!((hint_clips_eq, hint_nodes_eq), (1542, 1542));
    // our writer reproduces every loadable file byte for byte
    assert_eq!(reencode_diff, 0);
}
