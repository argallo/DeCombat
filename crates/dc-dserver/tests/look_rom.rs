//! ROM-gated check of DSV-90 (skipped without `CA_ROM`): no look the server sends in `GS_PLAYER_INFO` makes the
//! 2009 client attach a face row's client FX to a character. Every ROM value is read at run time.

use dc_bute::ButeDoc;
use dc_dserver::gs::{Look, DEFAULT_LOOK_BYTES, LOOK_OVERRIDE_BYTE};

/// Look byte 1: the character's model row (look object +0x00).
// EVIDENCE: cshell+0x1c2a90 (the 2nd look byte -> look +0x00, the 15th -> +0x13c)
const LOOK_MODEL_BYTE: usize = 1;

// OBSERVED: ROM table key MODELBUTES.TXT [Model<n>] FaceIndex, [Overidable_Model<n>] FaceIndex, [Face<n>] ClientFX<k>
const MODEL_ROW: &str = "Model";
const OVERRIDE_ROW: &str = "Overidable_Model";
const FACE_ROW: &str = "Face";
const KEY_FACE: &str = "FaceIndex";
const KEY_FACE_FX: &str = "ClientFX0";

/// The face row the client picks for a look: the model row's `FaceIndex`, replaced by the override row's when the
/// override byte is a row of the override table whose `FaceIndex` is set.
// EVIDENCE: cshell+0x1e3430 (face of the model row; the override wins when 0 <= override < the override row count
// and its face is not -1); ae-triage-reddot2-blast §1.3 step 4
fn face_of(doc: &ButeDoc, look: &[i8; 15]) -> i32 {
    let model = look[LOOK_MODEL_BYTE];
    let mut face = doc.get_int(&format!("{MODEL_ROW}{model}"), KEY_FACE, -1);
    let ov = look[LOOK_OVERRIDE_BYTE];
    let rows = (0..)
        .take_while(|n| doc.contains_key(&format!("{OVERRIDE_ROW}{n}")))
        .count() as i32;
    if ov >= 0 && i32::from(ov) < rows {
        let f = doc.get_int(&format!("{OVERRIDE_ROW}{ov}"), KEY_FACE, -1);
        if f != -1 {
            face = f;
        }
    }
    face
}

fn face_has_fx(doc: &ButeDoc, face: i32) -> bool {
    face >= 0
        && doc
            .get_str(&format!("{FACE_ROW}{face}"), KEY_FACE_FX)
            .is_some_and(|v| !v.trim().trim_matches('"').is_empty())
}

// DC-DEFAULT: DSV-90 (proposed)
// OBSERVED: capture 20261025_125432_rust6 (the bot's all-zero look; the real client's own look
// `0,0,0,0,18,1,0,0,0,1,1,-1,1,0,-1`); ae-triage-reddot2-blast §1.3
#[test]
fn sent_looks_select_no_face_with_client_fx() {
    let Some(rom) = dc_rez::dc_rom::test_rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let vfs = dc_rez::RezVfs::new(&rom);
    let Some(doc) = ButeDoc::from_vfs(
        &vfs,
        dc_dserver::rom_tables::ATTRIBUTES_REZ,
        dc_dserver::chars::MODELBUTES_ENTRY,
    ) else {
        println!("skipped: no character model table");
        return;
    };
    let raw = |bytes: [i8; 15]| Look {
        bytes,
        text_a: Vec::new(),
        text_b: Vec::new(),
    };
    let zero = raw([0; 15]);
    let user = raw([0, 0, 0, 0, 18, 1, 0, 0, 0, 1, 1, -1, 1, 0, -1]);
    let mut row0 = user.clone();
    row0.bytes[LOOK_OVERRIDE_BYTE] = 0;
    let sent = [zero.sanitised(), user.sanitised(), row0.sanitised()];
    assert_eq!(sent[0].bytes, DEFAULT_LOOK_BYTES);
    for l in &sent {
        let face = face_of(&doc, &l.bytes);
        assert!(
            !face_has_fx(&doc, face),
            "look {:?} selects face {face}, which has a client FX",
            l.bytes
        );
    }
    // the cause: the unsanitised zero look selects a face with a client FX (printed, not required)
    let face = face_of(&doc, &zero.bytes);
    println!(
        "unsanitised zero look: face {face}, client FX {}",
        face_has_fx(&doc, face)
    );
}

/// DSV-474 (user 2026-10-31: the bots' female faces were stretched): a player whose equipment names no vest (accounts
/// have none by default, LG-90) gets, for every face the creation screen offers, a body whose skeleton has every node
/// of the face by name (the body vest row 0 relates the face to; before, the male default body whatever the face, and
/// the female face nodes it lacks stayed in their bind pose while the body moved).
// OBSERVED: ROM table CREATEMODEL.TXT [Create_Model<n>] FacePieceId, ViewType (rows read at run time)
#[test]
fn rom_every_creation_face_gets_a_body_with_its_nodes() {
    let Some(rom) = dc_rez::dc_rom::test_rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let vfs = dc_rez::RezVfs::new(&rom);
    let Ok(t) = dc_dserver::chars::CharTables::from_vfs(&vfs) else {
        println!("skipped: no character model table");
        return;
    };
    let Some(create) = ButeDoc::from_vfs(
        &vfs,
        dc_dserver::rom_tables::ATTRIBUTES_REZ,
        "ATTRIBUTES\\CREATEMODEL.TXT",
    ) else {
        println!("skipped: no creation table");
        return;
    };
    let faces: Vec<u32> = (0..64)
        .map(|n| format!("Create_Model{n}"))
        .filter(|s| create.contains_key(s) && create.get_int(s, "ViewType", 0) == 0)
        .filter_map(|s| u32::try_from(create.get_int(&s, "FacePieceId", -1)).ok())
        .collect();
    assert!(faces.len() >= 2, "the creation table offers faces");
    let nodes = |path: &str| -> Vec<Vec<u8>> {
        let (rez, entry) = dc_dserver::chars::archive_of(path).expect("archive");
        let bytes = vfs.entry(&rez, &entry).expect("rez").expect("entry");
        dc_model::Model::parse_without_tracks(&bytes)
            .expect("model")
            .nodes
            .iter()
            .map(|n| n.name.0.to_ascii_uppercase())
            .collect()
    };
    // the bots' equipment: weapons, no vest, no character item
    let equipped = [(Some(0u8), 145u32), (Some(1), 164)];
    let default_row = dc_dserver::chars::CharacterChoice::default().model_row;
    for face in faces {
        let row = dc_dserver::chars::loadout_look(&t, &equipped, face)
            .0
            .unwrap_or(default_row);
        let (Some(b), Some(f)) = (t.bodies.get(&row), t.faces.get(&face)) else {
            panic!("face {face}: body row {row} or the face row is missing");
        };
        let (bn, fnodes) = (nodes(&b.model), nodes(&f.model));
        let missing = fnodes.iter().filter(|n| !bn.contains(n)).count();
        println!(
            "face {face}: body row {row}, face nodes {}, missing on the body {missing}",
            fnodes.len()
        );
        assert_eq!(
            missing, 0,
            "face {face} on body row {row}: {missing} of its nodes are not on the body"
        );
    }
}
