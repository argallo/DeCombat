use super::*;
use crate::hud::{Hud, HudDriver};
use crate::mods::{HostRecipe, RecipeMod};
use crate::world::RemoteBody;
use dc_mod_lua::{ModOrigin, ModPolicy};
use dc_sim::{Collider, Cylinder, Hit, V3};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The tests that install a mod set (the installed set is process-wide).
static INSTALL: Mutex<()> = Mutex::new(());

/// A wall across `z = 250` for `x > 0` (the right half of the view is behind it).
struct HalfWall;
impl Collider for HalfWall {
    fn sweep(&self, _: &Cylinder, from: V3, to: V3) -> Option<Hit> {
        if !(from.z < 250.0 && to.z >= 250.0) {
            return None;
        }
        let t = (250.0 - from.z) / (to.z - from.z);
        let x = from.x + (to.x - from.x) * t;
        (x > 0.0).then(|| Hit {
            t,
            normal: V3::from_array([0.0, 0.0, -1.0]),
            face_normal: V3::from_array([0.0, 0.0, -1.0]),
            surface_flags: 0,
            model: 0,
            poly: 0,
        })
    }
    fn overlaps(&self, _: &Cylinder, _: V3) -> bool {
        false
    }
}

fn body(obj: u16, team: u8, x: f32) -> RemoteBody {
    RemoteBody {
        obj,
        pid: obj as u8,
        name: format!("p{obj}"),
        team,
        pos: [x, 0.0, 500.0],
        half: [16.0, 50.0, 16.0],
        yaw: None,
        look: None,
    }
}

fn camera() -> dc_render::Camera {
    dc_render::Camera::looking(glam::Vec3::ZERO, 0.0, 0.0, 1.5, 1.2)
}

fn temp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("dc_client_xray_{}_{}", tag, std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A mod folder with `caps` and `main.lua` = `src`.
fn mod_dir(root: &Path, id: &str, kind: &str, caps: &str, src: &str) -> PathBuf {
    let d = root.join(id);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(
        d.join("manifest.json"),
        format!(
            r#"{{"id": "{id}", "name": "T", "version": "1.0.0", "entry": "main.lua", "kind": "{kind}", "capabilities": [{caps}], "description": "A test mod for the client's render bridge.", "author": "tests", "screenshots": ["screenshots/shot.png"]}}"#
        ),
    )
    .unwrap();
    std::fs::create_dir_all(d.join("screenshots")).unwrap();
    std::fs::write(
        d.join("screenshots").join("shot.png"),
        [0x89u8, b'P', b'N', b'G', 13, 10, 26, 10, 0, 0, 0, 0],
    )
    .unwrap();
    std::fs::write(d.join("main.lua"), src).unwrap();
    d
}

fn recipe(mods: &[(&Path, ModOrigin)], client_visual_allowed: bool) -> HostRecipe {
    HostRecipe {
        policy: ModPolicy::Modded {
            client_visual_allowed,
        },
        mods: mods
            .iter()
            .map(|(d, origin)| {
                let m = dc_mod_lua::ModManifest::from_json_str(
                    &std::fs::read_to_string(d.join("manifest.json")).unwrap(),
                )
                .unwrap();
                RecipeMod {
                    dir: d.to_path_buf(),
                    origin: *origin,
                    id: m.id.clone(),
                    version: m.version.to_string(),
                    kind: m.kind.to_string(),
                }
            })
            .collect(),
        settings: Default::default(),
    }
}

/// The demo mod of the repository.
fn hacking_mod() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/mods/hacking_mod")
}

fn driver() -> HudDriver {
    HudDriver::new(Hud::fallback(1024, 768, None))
}

#[test]
fn the_context_lists_players_with_distance_team_and_line_of_sight() {
    let mut ents = Vec::new();
    let remotes = [body(11, 1, 150.0), body(12, 2, -150.0)];
    entities(&remotes, [0.0; 3], Some((5, 2)), Some(&HalfWall), &|r| r.team != 2, &mut ents);
    assert_eq!(ents.len(), 3);
    assert!(ents[0].is_local && ents[0].id == 5 && ents[0].team == 2);
    assert!(!ents[0].is_enemy && ents[1].is_enemy && !ents[2].is_enemy);
    assert!(!ents[1].los, "behind the wall");
    assert!(ents[2].los, "in the open");
    assert!((ents[1].distance - (150.0f32 * 150.0 + 500.0 * 500.0).sqrt()).abs() < 0.01);
    let t = context(&ents);
    let list = t.get_str("entities").as_table().unwrap();
    let first = list.borrow().get_int(2).as_table().unwrap();
    let first = first.borrow();
    let keys: Vec<String> = first
        .entries()
        .iter()
        .map(|(k, _)| format!("{k:?}"))
        .collect();
    for k in [
        "id",
        "kind",
        "team",
        "is_enemy",
        "distance",
        "is_local",
        "is_visible_by_line_of_sight",
    ] {
        assert!(keys.iter().any(|x| x.contains(k)), "{k} in {keys:?}");
    }
    assert_eq!(first.get_str("kind").as_str(), Some("player"));
    assert_eq!(
        first.get_str("is_visible_by_line_of_sight").as_bool(),
        Some(false)
    );
}

#[test]
fn the_xray_demo_mod_draws_an_occluded_remote_in_a_headless_frame() {
    let _g = INSTALL.lock().unwrap_or_else(|e| e.into_inner());
    let r = recipe(&[(&hacking_mod(), ModOrigin::Server)], false);
    let mods = crate::modassets::MatchMods::open(Some(&r)).expect("the demo mod loads");
    let remotes = [body(11, 1, 150.0), body(12, 2, -150.0)];
    // vanilla frame: no bridge, no boxes
    let mut plain = driver();
    plain.set_entity_visibility(&remotes, &camera(), Some(&HalfWall), Some((5, 2)));
    assert!(plain.xray.is_none() && plain.state.xray.is_empty());
    let before = plain.image(true).unwrap();
    // the declared x-ray server's frame
    let mut d = driver();
    mods.wire(&mut d);
    assert!(
        d.xray.is_some(),
        "hooks.render + world.full_visibility: a bridge"
    );
    d.set_entity_visibility(&remotes, &camera(), Some(&HalfWall), Some((5, 2)));
    let x = d.xray.as_ref().unwrap();
    assert_eq!(x.calls, 1);
    assert_eq!(
        x.looks.len(),
        2,
        "the demo marks both remotes, not the own player"
    );
    // the hidden one: a silhouette + four outline bars; the open one: the outline only
    let fills = d.state.xray.len();
    assert_eq!(fills, 1 + 4 + 4, "{:?}", d.state.xray);
    let rect = body_rect(&camera(), remotes[0].pos, remotes[0].half).unwrap();
    let after = d.image(true).unwrap();
    let (cx, cy) = (
        (rect[0] + rect[2] / 2.0) as u32,
        (rect[1] + rect[3] / 2.0) as u32,
    );
    let (p0, p1) = (before.pixel(cx, cy), after.pixel(cx, cy));
    assert_ne!(p0, p1, "the occluded remote is drawn");
    assert!(p1[0] > p1[2], "the default primary, a red: {p1:?}");
    // the open one has no silhouette at its centre
    let open = body_rect(&camera(), remotes[1].pos, remotes[1].half).unwrap();
    let (ox, oy) = (
        (open[0] + open[2] / 2.0) as u32,
        (open[1] + open[3] / 2.0) as u32,
    );
    assert_eq!(before.pixel(ox, oy), after.pixel(ox, oy));
    // a second frame right after: the cached answer, same boxes (CLI-1635)
    d.set_entity_visibility(&remotes, &camera(), Some(&HalfWall), Some((5, 2)));
    assert_eq!(d.xray.as_ref().unwrap().calls, 1);
    assert_eq!(d.state.xray.len(), fills);
    // a frame a refresh later: one more call, same boxes
    d.state.now_ms += REFRESH_MS;
    d.set_entity_visibility(&remotes, &camera(), Some(&HalfWall), Some((5, 2)));
    assert_eq!(d.xray.as_ref().unwrap().calls, 2);
    assert_eq!(d.state.xray.len(), fills);
    drop(d);
    drop(mods);
    assert!(
        crate::modassets::active().is_none(),
        "the match end uninstalls the set"
    );
}

#[test]
fn the_hud_wiring_installs_a_fixture_set_and_nothing_for_vanilla() {
    let _g = INSTALL.lock().unwrap_or_else(|e| e.into_inner());
    let root = temp("wire");
    let m = mod_dir(
        &root,
        "green",
        "shared",
        r#""content.hud", "hooks.hud""#,
        r#"return function(mod)
            mod.content.hud:override("crosshair_colour", "4080ff")
            mod.hooks:wrap("hud.draw", function(base, ctx) return base(ctx) end)
        end"#,
    );
    // vanilla: no recipe, nothing installed, the HUD untouched
    assert!(crate::modassets::MatchMods::open(None).is_none());
    assert!(crate::modassets::active().is_none());
    let r = recipe(&[(&m, ModOrigin::Server)], false);
    let mods = crate::modassets::MatchMods::open(Some(&r)).unwrap();
    assert!(crate::modassets::active().is_some_and(|a| !a.is_empty()));
    let mut d = driver();
    mods.wire(&mut d);
    assert_eq!(
        d.hud.overrides.crosshair_colour,
        Some([0x40, 0x80, 0xff, 0xff])
    );
    assert!(d.hud.hook.is_some(), "hooks.hud: the hud.draw bridge");
    assert!(d.xray.is_none(), "no hooks.render: no render bridge");
    drop(mods);
    assert!(crate::modassets::active().is_none());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_local_mod_never_gets_the_render_hook() {
    let root = temp("local");
    let src = r#"return function(mod)
        mod.hooks:wrap("render.entity_visibility", function(base, ctx) return base(ctx) end)
    end"#;
    let local = mod_dir(&root, "peek", "client_visual", r#""hooks.render""#, src);
    // the host refuses it as a local mod
    let mut host = ModHost::new();
    host.set_policy(ModPolicy::Modded {
        client_visual_allowed: true,
    });
    let e = host.load_mod_dir_as(ModOrigin::Local, &local).unwrap_err();
    assert!(e.contains("requires a mod declared by the server"), "{e}");
    // through a recipe the local mod is skipped, so no bridge exists
    let r = recipe(&[(&local, ModOrigin::Local)], true);
    let h = r.load().unwrap();
    assert!(h.load_order().is_empty());
    assert!(!h.has_hook(ENTITY_VISIBILITY_HOOK));
    assert!(LuaRenderHook::new(Rc::new(RefCell::new(h))).is_none());
    // a server mod with the hook but without world.full_visibility gets no bridge either
    let half = mod_dir(&root, "half", "shared", r#""hooks.render""#, src);
    let h = recipe(&[(&half, ModOrigin::Server)], false).load().unwrap();
    assert!(h.has_hook(ENTITY_VISIBILITY_HOOK));
    assert!(LuaRenderHook::new(Rc::new(RefCell::new(h))).is_none());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_lua_error_turns_the_render_hook_off_for_the_match() {
    let root = temp("err");
    let m = mod_dir(
        &root,
        "boom",
        "shared",
        r#""hooks.render", "world.full_visibility""#,
        r#"return function(mod)
            mod.hooks:wrap("render.entity_visibility", function(base, ctx) error("boom") end)
        end"#,
    );
    let h = recipe(&[(&m, ModOrigin::Server)], false).load().unwrap();
    let mut d = driver();
    d.xray = LuaRenderHook::new(Rc::new(RefCell::new(h)));
    let remotes = [body(11, 1, 150.0)];
    d.set_entity_visibility(&remotes, &camera(), Some(&HalfWall), None);
    let x = d.xray.as_ref().unwrap();
    assert_eq!(x.calls, 1);
    assert!(
        x.disabled().is_some_and(|w| w.contains("boom")),
        "{:?}",
        x.disabled()
    );
    d.set_entity_visibility(&remotes, &camera(), Some(&HalfWall), None);
    assert_eq!(d.xray.as_ref().unwrap().calls, 1, "no call after the error");
    assert!(d.state.xray.is_empty());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn malformed_answers_and_unknown_ids_are_ignored() {
    let ents = [EntityCtx {
        id: 11,
        team: 1,
        distance: 10.0,
        is_enemy: true,
        is_local: false,
        los: false,
    }];
    let mut out = Vec::new();
    let mut list = LuaTable::new();
    let entry = |id: f64, colour: LuaValue| {
        let mut t = LuaTable::new();
        t.set_str("id", LuaValue::Number(id));
        t.set_str("visible", LuaValue::Bool(true));
        // CLI-1635: the old colour field of an answer changes nothing
        t.set_str("tint_rgba", colour);
        LuaValue::Table(Rc::new(RefCell::new(t)))
    };
    list.set_int(1, entry(99.0, LuaValue::Nil));
    list.set_int(2, LuaValue::String("x".into()));
    list.set_int(3, entry(11.0, LuaValue::Number(0x40A0FFFFu32 as f64)));
    let mut t = LuaTable::new();
    t.set_str("entities", LuaValue::Table(Rc::new(RefCell::new(list))));
    parse_looks(&LuaValue::Table(Rc::new(RefCell::new(t))), &ents, &mut out);
    assert_eq!(
        out,
        vec![EntityLook {
            id: 11,
            visible: true,
            outline: false,
            mode: None,
        }]
    );
    parse_looks(&LuaValue::Nil, &ents, &mut out);
    assert!(out.is_empty());
}

fn ent(id: u16, los: bool) -> EntityCtx {
    EntityCtx {
        is_enemy: true,
        id,
        team: 1,
        distance: 10.0,
        is_local: false,
        los,
    }
}

fn look(id: u16, visible: bool, outline: bool) -> EntityLook {
    EntityLook {
        id,
        visible,
        outline,
        mode: None,
    }
}

const STYLE: XrayStyle = XrayStyle {
    mode: XrayMode::Box,
    primary: [0x20, 0xc0, 0x40, 0xf0],
    secondary: [0x10, 0x20, 0xff, 0xff],
    nametags: false,
    health: false,
    teammates: true,
};

/// The team mates' colours of the tests below.
const MATES: [[u8; 4]; 2] = [[1, 2, 3, 255], [4, 5, 6, 255]];

#[test]
fn the_box_style_uses_the_settings_colours() {
    let ents = [ent(11, false), ent(12, true)];
    let looks = [look(11, true, true), look(12, true, true)];
    let rect = |_: u16| Some([100.0, 100.0, 40.0, 80.0]);
    let (mut items, mut draws) = (Vec::new(), Vec::new());
    push_items(&looks, &ents, &STYLE, MATES, &rect, &mut items, &mut draws);
    assert!(draws.is_empty(), "box: HUD items only");
    // the hidden one: fill + 4 border bars; the one in sight: the border only
    assert_eq!(items.len(), 1 + 4 + 4);
    let colours: Vec<[u8; 4]> = items
        .iter()
        .map(|i| match i {
            DrawItem::Fill { colour, .. } => *colour,
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(
        colours[0],
        [0x20, 0xc0, 0x40, 0xf0 / 3],
        "fill = primary, alpha / 3"
    );
    assert!(
        colours[1..].iter().all(|c| *c == STYLE.secondary),
        "border = secondary"
    );
    // a team mate takes the other colours
    let mate = [EntityCtx {
        is_enemy: false,
        ..ent(11, false)
    }];
    items.clear();
    push_items(&looks[..1], &mate, &STYLE, MATES, &rect, &mut items, &mut draws);
    assert!(matches!(
        items[0],
        DrawItem::Fill {
            colour: [1, 2, 3, 85],
            ..
        }
    ));
    assert!(matches!(items[1], DrawItem::Fill { colour, .. } if colour == MATES[1]));
}

#[test]
fn chams_and_skeleton_become_renderer_draws_and_a_mod_mode_pins_the_style() {
    let ents = [ent(11, false), ent(12, true), ent(13, true)];
    let looks = [
        look(11, true, false),
        look(12, true, true),
        look(13, true, false),
    ];
    let rect = |_: u16| Some([100.0, 100.0, 40.0, 80.0]);
    let (mut items, mut draws) = (Vec::new(), Vec::new());
    // chams: every player the hook marks visible, hidden or in sight (user 2026-11-01: the colours never switch off)
    let chams = XrayStyle {
        mode: XrayMode::Chams,
        ..STYLE
    };
    push_items(&looks, &ents, &chams, MATES, &rect, &mut items, &mut draws);
    assert!(items.is_empty());
    assert_eq!(
        draws,
        [11, 12, 13]
            .map(|entity| XrayDraw {
                entity,
                mode: XrayMode::Chams,
                primary: STYLE.primary,
                secondary: STYLE.secondary,
            })
            .to_vec()
    );
    // skeleton: the hidden one and the outlined one
    draws.clear();
    let skel = XrayStyle {
        mode: XrayMode::Skeleton,
        ..STYLE
    };
    push_items(&looks, &ents, &skel, MATES, &rect, &mut items, &mut draws);
    assert_eq!(
        draws.iter().map(|d| d.entity).collect::<Vec<_>>(),
        vec![11, 12]
    );
    assert!(draws.iter().all(|d| d.mode == XrayMode::Skeleton));
    // the mod's mode pins over the setting; the colours stay the player's
    let pinned = [EntityLook {
        mode: Some(XrayMode::Chams),
        ..look(11, true, false)
    }];
    draws.clear();
    push_items(&pinned, &ents, &STYLE, MATES, &rect, &mut items, &mut draws);
    assert!(items.is_empty(), "pinned chams: no box");
    assert_eq!(draws[0].mode, XrayMode::Chams);
    assert_eq!(draws[0].primary, STYLE.primary);
    // the answer's `mode` is parsed (an unknown name: the setting)
    let mut list = LuaTable::new();
    for (i, m) in ["skeleton", "glow"].iter().enumerate() {
        let mut t = LuaTable::new();
        t.set_str("id", LuaValue::Number(11.0 + i as f64));
        t.set_str("visible", LuaValue::Bool(true));
        t.set_str("mode", LuaValue::String((*m).into()));
        list.set_int(i as i64 + 1, LuaValue::Table(Rc::new(RefCell::new(t))));
    }
    let mut t = LuaTable::new();
    t.set_str("entities", LuaValue::Table(Rc::new(RefCell::new(list))));
    let mut out = Vec::new();
    parse_looks(&LuaValue::Table(Rc::new(RefCell::new(t))), &ents, &mut out);
    assert_eq!(out[0].mode, Some(XrayMode::Skeleton));
    assert_eq!(out[1].mode, None);
}

#[test]
fn the_demo_server_hands_chams_draws_to_the_renderer() {
    let _g = INSTALL.lock().unwrap_or_else(|e| e.into_inner());
    let r = recipe(&[(&hacking_mod(), ModOrigin::Server)], false);
    let mods = crate::modassets::MatchMods::open(Some(&r)).expect("the demo mod loads");
    let remotes = [body(11, 1, 150.0), body(12, 2, -150.0)];
    let mut d = driver();
    mods.wire(&mut d);
    d.xray_style = XrayStyle {
        mode: XrayMode::Chams,
        ..XrayStyle::default()
    };
    d.set_entity_visibility(&remotes, &camera(), Some(&HalfWall), Some((5, 2)));
    // both remotes are drawn as chams in the player's colours (the one in sight too); no boxes; no own team known yet
    assert!(d.state.xray.is_empty(), "{:?}", d.state.xray);
    assert_eq!(d.xray_draws.len(), 2);
    assert_eq!(d.xray_draws[0].entity, 11);
    assert_eq!(d.xray_draws[0].primary, crate::settings::XRAY_PRIMARY_DEFAULT);
    assert_eq!(d.xray_draws[1].entity, 12);
    assert_eq!(d.xray_draws[1].primary, crate::settings::XRAY_PRIMARY_DEFAULT);
    drop(d);
    drop(mods);
}

#[test]
fn wallhack_cycles_the_style_and_the_profile_keeps_it() {
    let _g = INSTALL.lock().unwrap_or_else(|e| e.into_inner());
    use crate::hud::chat::{ChatPress, LocalCommand};
    let typed = |d: &mut HudDriver| {
        d.chat_press(ChatPress::Enter);
        d.chat_press(ChatPress::Text("/wallhack".into()));
        let o = d.chat_press(ChatPress::Enter);
        assert_eq!(o.command, Some(LocalCommand::Wallhack));
        assert!(o.send.is_none(), "never sent");
    };
    let last_line = |d: &HudDriver| d.state.chat.back().map(|l| l.text.clone());
    // no render bridge: nothing changes, the notice says why
    let mut plain = driver();
    plain.xray_style = XrayStyle::default();
    let _ = crate::settings::take_xray_note();
    typed(&mut plain);
    assert_eq!(plain.xray_style.mode, XrayMode::Box);
    assert_eq!(
        last_line(&plain).as_deref(),
        Some(crate::hud::WALLHACK_NO_MOD)
    );
    assert_eq!(crate::settings::take_xray_note(), None);
    // a declared x-ray server: box -> chams -> skeleton -> box, each noted for the menu to save
    let root = temp("wallhack");
    let m = mod_dir(
        &root,
        "xr",
        "shared",
        r#""hooks.render", "world.full_visibility""#,
        "return function(mod) end",
    );
    let h = recipe(&[(&m, ModOrigin::Server)], false).load().unwrap();
    let mut d = driver();
    d.xray = LuaRenderHook::new(Rc::new(RefCell::new(h)));
    d.xray_style = XrayStyle::default();
    for want in [
        XrayMode::Chams,
        XrayMode::Skeleton,
        XrayMode::Box,
        XrayMode::Chams,
    ] {
        typed(&mut d);
        assert_eq!(d.xray_style.mode, want);
        assert_eq!(last_line(&d), Some(format!("Wallhack: {}", want.name())));
        assert_eq!(
            crate::settings::take_xray_note().map(|s| s.mode),
            Some(want)
        );
    }
    assert_eq!(crate::settings::xray_style().mode, XrayMode::Chams);
    // the profile round trip keeps the style
    let s = crate::settings::Settings {
        xray: XrayStyle {
            mode: XrayMode::Skeleton,
            primary: [1, 2, 3, 4],
            secondary: [0xaa, 0xbb, 0xcc, 0xdd],
            nametags: true,
            health: true,
            teammates: false,
        },
        ..crate::settings::Settings::default()
    };
    let table = crate::settings::CommandTable { rows: Vec::new() };
    let names = |_: i32| String::new();
    let mut doc = crate::settings::ProfileDoc::parse("");
    s.to_doc(&mut doc, &table, &names);
    let back = crate::settings::Settings::from_doc(
        &crate::settings::ProfileDoc::parse(&doc.text()),
        &table,
        &names,
    );
    assert_eq!(back.xray, s.xray);
    // a malformed value keeps the default
    let mut doc = crate::settings::ProfileDoc::parse("");
    doc.own_set(crate::settings::OWN_XRAY_MODE, "glow");
    doc.own_set(crate::settings::OWN_XRAY_PRIMARY, "zz");
    let back = crate::settings::Settings::from_doc(&doc, &table, &names);
    assert_eq!(back.xray, XrayStyle::default());
    crate::settings::note_xray_style(XrayStyle::default());
    let _ = crate::settings::take_xray_note();
    let _ = std::fs::remove_dir_all(&root);
}

/// The in-test crosshair mod (a client_visual mod written to a temp folder; the texture is generated).
fn test_crosshair_mod() -> PathBuf {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let root = temp(&format!("crosshair_fixture{}", N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
    crate::modassets::test_fixture::write_crosshair_mod(&root, "test_crosshair")
}

/// CLI-1609 / CLI-1613 (playtest report: the x-ray boxes showed, the crosshair stayed the ROM one): the live
/// modded set [hacking_mod (shared), test_crosshair (client_visual)], both declared by the server, under a policy
/// with `client_visual_allowed = false`, draws the mod's crosshair texture in the mod's colour.
#[test]
fn the_declared_test_crosshair_draws_in_a_headless_frame_beside_the_xray_mod() {
    let _g = INSTALL.lock().unwrap_or_else(|e| e.into_inner());
    let r = recipe(
        &[
            (&hacking_mod(), ModOrigin::Server),
            (&test_crosshair_mod(), ModOrigin::Server),
        ],
        false,
    );
    let mods = crate::modassets::MatchMods::open(Some(&r)).expect("the declared set loads");
    assert_eq!(
        mods.overrides.crosshair.as_deref(),
        Some("@mod:test_crosshair/assets/crosshair.dtx")
    );
    assert_eq!(mods.overrides.crosshair_colour, Some([0x40, 0x80, 0xff, 0xff]));
    let alive = |d: &mut HudDriver| {
        d.state.life_state = 1;
        d.state.health = Some(100.0);
        d.state.own_pid = Some(0);
    };
    let mut plain = driver();
    alive(&mut plain);
    let before = plain.image(true).unwrap();
    let mut d = driver();
    alive(&mut d);
    mods.wire(&mut d);
    // the HUD open's log line names what is applied, and the texture loaded
    let (line, failed) = crate::modassets::describe_hud_overrides(&d.hud.overrides, &mut d.hud.assets);
    assert_eq!(
        line,
        "mods: hud overrides: crosshair=@mod:test_crosshair/assets/crosshair.dtx (32x32), colour=#4080ffff"
    );
    assert!(failed.is_empty(), "{failed:?}");
    let items = d.hud.items(&d.state);
    let img = items.iter().find_map(|i| match i {
        crate::hud::DrawItem::Image { tex, dst, tint, .. } if tex.starts_with("@mod:") => {
            Some((tex.clone(), *dst, *tint))
        }
        _ => None,
    });
    let (tex, dst, tint) = img.expect("the crosshair element draws the mod's texture");
    assert_eq!(tex, "@mod:test_crosshair/assets/crosshair.dtx");
    assert_eq!(tint, [0x40, 0x80, 0xff, 0xff]);
    assert!(
        !items
            .iter()
            .any(|i| matches!(i, crate::hud::DrawItem::AimCross { .. })),
        "the procedural cross is replaced"
    );
    let after = d.image(true).unwrap();
    // the texture's opaque pixels, sampled across its box: some differ from the plain frame and are blue
    let mut blue = 0;
    for y in (dst[1] as u32)..((dst[1] + dst[3]) as u32) {
        for x in (dst[0] as u32)..((dst[0] + dst[2]) as u32) {
            let (p0, p1) = (before.pixel(x, y), after.pixel(x, y));
            if p0 != p1 && p1[2] > p1[0] + 64 && p1[2] > p1[1] + 32 {
                blue += 1;
            }
        }
    }
    assert!(blue > 8, "the mod's crosshair shows blue pixels: {blue}");
    drop(d);
    // a wide target: the reticle stays round (as wide as tall in pixels), as large as the width scale makes it
    let mut wide = HudDriver::new(Hud::fallback(1920, 1080, None));
    alive(&mut wide);
    let plain = wide.image(true).unwrap();
    mods.wire(&mut wide);
    let shot = wide.image(true).unwrap();
    let (mut lo, mut hi) = ([u32::MAX; 2], [0u32; 2]);
    for y in 0..1080 {
        for x in 0..1920 {
            let p = shot.pixel(x, y);
            if p != plain.pixel(x, y) && p[2] > p[0] + 64 && p[2] > p[1] + 32 {
                lo = [lo[0].min(x), lo[1].min(y)];
                hi = [hi[0].max(x), hi[1].max(y)];
            }
        }
    }
    let (w, h) = (hi[0] - lo[0] + 1, hi[1] - lo[1] + 1);
    assert!(w.abs_diff(h) <= 2, "round on 16:9: {w} x {h}");
    assert!((40..=50).contains(&w), "the ring's 24 texels at 1920 / 1024: {w}");
    // the ROM HUD of a match with a rifle in hand: the weapon's aim mark is the crosshair element
    if let Some(rom) = dc_rom::test_rom() {
        let hud = Hud::from_rom(&rom, 1024, 768, None).expect("ROM HUD");
        let mut d = HudDriver::new(hud);
        alive(&mut d);
        d.state.aim_mark = Some(crate::spread::AimMarkView {
            shape: crate::spread::mark::SHAPE_PLAIN,
            distance: 10.0,
            since_shot_s: None,
            colour: [0, 255, 0, 255],
            rest: 10.0,
        });
        mods.wire(&mut d);
        let items = d.hud.items(&d.state);
        assert!(
            !items
                .iter()
                .any(|i| matches!(i, crate::hud::DrawItem::AimMark(_))),
            "the aim mark is replaced"
        );
        assert!(
            items.iter().any(|i| matches!(i, crate::hud::DrawItem::Image { tex, tint, .. }
                if tex == "@mod:test_crosshair/assets/crosshair.dtx" && *tint == [0x40, 0x80, 0xff, 0xff])),
            "the ROM HUD draws the mod's crosshair"
        );
    }
    drop(mods);
}

/// CLI-1613 (playtest report, fourth time "still green"; the live log named radar pictures, fills and texts as
/// the crosshair element): the live match's HUD has a radar frame, and the radar's items go to the FRONT of the list
/// (drawn first), so the element marks taken before them pointed at the wrong items and the override missed the aim
/// mark. The live case: the HUD at 2560 x 1440 with a radar frame and a weapon's aim mark, both demo mods declared.
#[test]
fn the_declared_test_crosshair_replaces_the_aim_mark_while_the_radar_draws() {
    let _g = INSTALL.lock().unwrap_or_else(|e| e.into_inner());
    let r = recipe(
        &[
            (&hacking_mod(), ModOrigin::Server),
            (&test_crosshair_mod(), ModOrigin::Server),
        ],
        true,
    );
    let mods = crate::modassets::MatchMods::open(Some(&r)).expect("the declared set loads");
    let (w, h) = (2560u32, 1440u32);
    let mut huds = vec![("fallback", Hud::fallback(w, h, None))];
    if let Some(rom) = dc_rom::test_rom() {
        huds.push(("rom", Hud::from_rom(&rom, w, h, None).expect("ROM HUD")));
    }
    for (name, hud) in huds {
        let mut d = HudDriver::new(hud);
        mods.wire(&mut d);
        d.state.life_state = 1;
        d.state.health = Some(100.0);
        d.state.own_pid = Some(0);
        d.state.now_ms = 5000;
        d.state.aim_mark = Some(crate::spread::AimMarkView {
            shape: crate::spread::mark::SHAPE_PLAIN,
            distance: 10.0,
            since_shot_s: None,
            colour: [0, 255, 0, 255],
            rest: 10.0,
        });
        d.state.radar = Some(crate::hud::radar::RadarFrame {
            own: [0.0; 3],
            yaw: 0.0,
            blips: Vec::new(),
        });
        let items = d.hud.items(&d.state);
        assert!(
            d.hud.radar.is_none() || items.len() > 2,
            "{name}: the radar draws items"
        );
        assert!(
            !items.iter().any(|i| matches!(
                i,
                crate::hud::DrawItem::AimMark(_) | crate::hud::DrawItem::AimCross { .. }
            )),
            "{name}: the stock aim mark is replaced while the radar draws"
        );
        assert!(
            items.iter().any(|i| matches!(i, crate::hud::DrawItem::Image { tex, tint, .. }
                if tex == "@mod:test_crosshair/assets/crosshair.dtx" && *tint == [0x40, 0x80, 0xff, 0xff])),
            "{name}: the mod's blue crosshair draws while the radar draws"
        );
        // the log names the crosshair units by kind, wherever they sit in the list
        let (kind, line) = crate::hud::modhud::crosshair_drawn(
            &items,
            d.hud.overrides.crosshair.as_deref(),
            (w, h),
        )
        .expect("a crosshair unit");
        assert_eq!(
            kind, "image @mod:test_crosshair/assets/crosshair.dtx",
            "{name}: {line}"
        );
        assert_eq!(
            line,
            "mods: hud crosshair drawn as image @mod:test_crosshair/assets/crosshair.dtx tint #4080ffff at 1240,680 80x80 px",
            "{name}"
        );
    }
    drop(mods);
}

/// CLI-1613 (playtest report, third time "the crosshair is still green"): the declared blue crosshair through
/// the match's REAL HUD path: the GPU overlay (`HudDriver::items_due` -> `OverlayRenderer::update_items`) over a
/// rendered frame, read back. The centre shows the mod's blue and none of the stock green. Skips without an adapter.
#[test]
fn the_declared_test_crosshair_draws_on_the_gpu_hud_layer() {
    use crate::world::GpuTarget;
    use crate::WorldSource;
    let _g = INSTALL.lock().unwrap_or_else(|e| e.into_inner());
    let ctx = match dc_render::GpuContext::headless(dc_render::AdapterChoice::Auto) {
        Ok(c) => c,
        Err(e) => {
            println!("skipped: no GPU adapter ({e})");
            return;
        }
    };
    let (w, h) = (1280u32, 720u32);
    let renderer = dc_render::WorldRenderer::new(
        &ctx,
        dc_render::OFFSCREEN_FORMAT,
        dc_render::RenderSettings::default(),
    );
    let world = crate::testkit::synth_source()
        .load(Some(GpuTarget {
            ctx: &ctx,
            renderer: &renderer,
        }))
        .expect("synthetic world");
    let camera = dc_render::Camera::looking(glam::Vec3::ZERO, 0.0, 0.0, 1.5, 1.2);
    let target = crate::offscreen::Offscreen::new(&ctx, dc_render::OFFSCREEN_FORMAT, w, h);
    let r = recipe(
        &[
            (&hacking_mod(), ModOrigin::Server),
            (&test_crosshair_mod(), ModOrigin::Server),
        ],
        true,
    );
    let mods = crate::modassets::MatchMods::open(Some(&r)).expect("the declared set loads");
    // the match's HUD as `window::start` opens it: the ROM HUD when there is a ROM, the fallback otherwise
    let hud = match dc_rom::test_rom() {
        Some(rom) => Hud::from_rom(&rom, w, h, None).expect("ROM HUD"),
        None => Hud::fallback(w, h, None),
    };
    let mut d = HudDriver::new(hud);
    mods.wire(&mut d);
    d.state.life_state = 1;
    d.state.health = Some(100.0);
    d.state.own_pid = Some(0);
    // a rifle in hand: the weapon's green aim mark is the crosshair element
    d.state.aim_mark = Some(crate::spread::AimMarkView {
        shape: crate::spread::mark::SHAPE_PLAIN,
        distance: 10.0,
        since_shot_s: None,
        colour: [0, 255, 0, 255],
        rest: 5.0,
    });
    // the match's radar draws (its items go first in the list; the live "still green" case)
    d.state.radar = Some(crate::hud::radar::RadarFrame {
        own: [0.0; 3],
        yaw: 0.0,
        blips: Vec::new(),
    });
    let mut layer = crate::overlay::OverlayRenderer::new(&ctx, dc_render::OFFSCREEN_FORMAT);
    let items = d.items_due(true).expect("a list");
    layer.update_items(&ctx, &items, &mut d.hud.assets, (w, h));
    let frame = target
        .render_with_hud(
            &ctx,
            &renderer,
            world.scene.as_ref().unwrap(),
            &camera,
            None,
            &[],
            &[],
            Some(&layer),
            None,
        )
        .unwrap();
    let (mut blue, mut green) = (0, 0);
    for y in (h / 2 - 40)..(h / 2 + 40) {
        for x in (w / 2 - 40)..(w / 2 + 40) {
            let o = ((y * w + x) * 4) as usize;
            let p = &frame.rgba[o..o + 4];
            if p[2] > p[0].saturating_add(64) && p[2] > p[1].saturating_add(32) {
                blue += 1;
            }
            if p[1] > 200 && p[0] < 60 && p[2] < 60 {
                green += 1;
            }
        }
    }
    println!("centre: {blue} blue, {green} stock-green pixels");
    assert!(
        blue > 20,
        "the mod's blue crosshair on the GPU layer: {blue}"
    );
    assert_eq!(green, 0, "no stock green aim mark on the GPU layer");
    drop(layer);
    drop(mods);
}

/// CLI-1631: the test's name-tag font.
const TAG: crate::hud::layout::TextStyle = crate::hud::layout::TextStyle {
    font: 3,
    colour: [1, 1, 1, 255],
    size: Some(14),
};

fn info(health: Option<(u16, u16)>, tag_at: Option<[f32; 2]>, alive: bool) -> PlayerInfo {
    PlayerInfo {
        health,
        tag_at,
        alive,
    }
}

/// CLI-1631 (amended 2026-11-01): `push_info` draws the bars only; no name text of ours. A bar stands over the
/// player's 2009 tag when one is shown, else over the body box.
#[test]
fn health_bars_draw_over_visible_players_above_their_2009_tag() {
    let ents = [ent(11, false), ent(12, true), ent(13, false)];
    // 11 hidden + visible, 12 in sight + visible, 13 not marked visible
    let looks = [
        look(11, true, false),
        look(12, true, false),
        look(13, false, true),
    ];
    let rect = |_: u16| Some([100.0, 200.0, 40.0, 80.0]);
    let known = |_: u16| Some(info(Some((25, 100)), None, true));
    let both = XrayStyle {
        nametags: true,
        health: true,
        ..STYLE
    };
    let mut out = Vec::new();
    push_info(&looks, &ents, &both, MATES, TAG, &rect, &known, &mut out);
    // per visible player: the bar's back and its fill, never a label
    assert_eq!(out.len(), 2 * 2, "{out:?}");
    assert!(out.iter().all(|i| matches!(i, DrawItem::Fill { .. })), "{out:?}");
    let bar_y = 200.0 - INFO_GAP - BAR_H;
    assert_eq!(
        out[0],
        DrawItem::Fill {
            rect: [120.0 - BAR_W / 2.0, bar_y, BAR_W, BAR_H],
            colour: BAR_BACK
        }
    );
    assert_eq!(
        out[1],
        DrawItem::Fill {
            rect: [120.0 - BAR_W / 2.0, bar_y, BAR_W / 4.0, BAR_H],
            colour: STYLE.primary
        }
    );
    // a player with his 2009 tag at (300, 150): the bar centred over the caption line (14 px)
    let tagged = |_: u16| Some(info(Some((100, 100)), Some([300.0, 150.0]), true));
    out.clear();
    push_info(&looks, &ents, &both, MATES, TAG, &rect, &tagged, &mut out);
    assert_eq!(
        out[0],
        DrawItem::Fill {
            rect: [300.0 - BAR_W / 2.0, 150.0 - 14.0 - INFO_GAP - BAR_H, BAR_W, BAR_H],
            colour: BAR_BACK
        }
    );
    // names only, or both switches off: push_info draws nothing (the tags are the 2009 marks)
    for st in [
        XrayStyle {
            nametags: true,
            ..STYLE
        },
        STYLE,
    ] {
        out.clear();
        push_info(&looks, &ents, &st, MATES, TAG, &rect, &known, &mut out);
        assert!(out.is_empty(), "{out:?}");
    }
}

#[test]
fn an_unknown_health_draws_the_bar_outline_only_and_the_dead_get_no_bar() {
    let ents = [ent(11, false)];
    let looks = [look(11, true, false)];
    let rect = |_: u16| Some([100.0, 200.0, 40.0, 80.0]);
    let unknown = |_: u16| Some(info(None, None, true));
    let bars = XrayStyle {
        health: true,
        ..STYLE
    };
    let mut out = Vec::new();
    push_info(&looks, &ents, &bars, MATES, TAG, &rect, &unknown, &mut out);
    assert_eq!(out.len(), 4, "four 1 px edges: {out:?}");
    assert!(out.iter().all(|i| matches!(i, DrawItem::Fill { colour, rect }
        if *colour == STYLE.primary && (rect[2] == 1.0 || rect[3] == 1.0))));
    // a player who is not alive, or gone (no info): no bar at all
    for gone in [Some(info(Some((0, 100)), None, false)), None] {
        out.clear();
        push_info(&looks, &ents, &bars, MATES, TAG, &rect, &|_| gone, &mut out);
        assert!(out.is_empty(), "{out:?}");
    }
}

/// The driver's board for the tests below: own player 5 on team 2, p11 an enemy (team 1), p12 a mate (team 2).
fn board(d: &mut HudDriver) {
    d.state.own_pid = Some(5);
    for (pid, team) in [(5u8, 2u8), (11, 1), (12, 2)] {
        d.state.roster.insert(
            pid,
            crate::hud::state::BoardRow {
                pid,
                name: format!("p{pid}"),
                team,
                ..Default::default()
            },
        );
    }
}

fn kill(victim: u8) -> crate::hud::state::HudEvent {
    crate::hud::state::HudEvent::Kill(dc_botclient::shell::KillRecord {
        victim,
        killer: 5,
        weapon: 145,
        hit_zone: 3,
        complete: true,
        ..Default::default()
    })
}

fn health_of(pid: u8, health: u16) -> crate::hud::state::HudEvent {
    crate::hud::state::HudEvent::RemoteHealth(vec![dc_wire::remotehealth::HealthEntry {
        pid,
        health,
        max: 100,
    }])
}

/// The 2009 tags of the frame as `(pid, state)`.
fn tags(d: &HudDriver) -> Vec<(u8, crate::hud::marks::MarkState)> {
    d.state.marks.iter().map(|m| (m.pid, m.state)).collect()
}

/// The bars of the frame: the dark backs in the x-ray layer.
fn bars(d: &HudDriver) -> usize {
    d.state
        .xray
        .iter()
        .filter(|i| matches!(i, DrawItem::Fill { colour, .. } if *colour == BAR_BACK))
        .count()
}

/// CLI-1631 (amended, project decision): on the demo server with the switches on, the visible enemy gets the
/// 2009 enemy tag as if aimed at (a mark of state Enemy, no label of ours in the x-ray layer); the mate keeps his
/// 2009 mate tag; the bars come from the DSV-480 health; a "No-recoil only" room drops the bridge, so nothing is
/// forced and no bar is drawn.
#[test]
fn the_driver_forces_the_2009_enemy_tag_and_draws_bars_and_a_no_recoil_only_room_draws_nothing() {
    use crate::hud::marks::MarkState;
    let _g = INSTALL.lock().unwrap_or_else(|e| e.into_inner());
    let r = recipe(&[(&hacking_mod(), ModOrigin::Server)], false);
    let mods = crate::modassets::MatchMods::open(Some(&r)).expect("the demo mod loads");
    let remotes = [body(11, 1, 150.0), body(12, 2, -150.0)];
    let on = XrayStyle {
        nametags: true,
        health: true,
        ..XrayStyle::default()
    };
    let mut d = driver();
    mods.wire(&mut d);
    board(&mut d);
    d.xray_style = on;
    d.state.apply(&health_of(11, 50), 0, 0.0);
    d.set_marks(&remotes, &camera(), None);
    assert_eq!(tags(&d), [(12, MarkState::Mate)], "no cross on p11: the 2009 rule alone");
    d.set_entity_visibility(&remotes, &camera(), Some(&HalfWall), Some((5, 2)));
    assert_eq!(
        tags(&d),
        [(11, MarkState::Enemy), (12, MarkState::Mate)],
        "the enemy's 2009 tag is forced on"
    );
    assert!(
        !d.state.xray.iter().any(|i| matches!(i, DrawItem::Label { .. })),
        "no name of ours: {:?}",
        d.state.xray
    );
    // p11's bar: back + half fill, over his tag; p12's health is unknown: the outline
    let half_fill = d.state.xray.iter().any(|i| {
        matches!(i, DrawItem::Fill { rect, .. }
            if (rect[2] - BAR_W / 2.0).abs() < 1e-3 && rect[3] == BAR_H)
    });
    assert!(half_fill, "{:?}", d.state.xray);
    let tag11 = d.state.marks[0].at;
    assert!(d.state.xray.iter().any(|i| matches!(i, DrawItem::Fill { rect, colour }
        if *colour == BAR_BACK
            && (rect[0] + BAR_W / 2.0 - tag11[0]).abs() < 1e-3
            && rect[1] + BAR_H < tag11[1])));
    // the next frame's marks keep it (the forced set is the hook's last answer)
    d.set_marks(&remotes, &camera(), None);
    assert_eq!(tags(&d).len(), 2);
    // the name-tag switch off: the enemy's tag goes back to the 2009 rule (under the cross only)
    d.xray_style.nametags = false;
    d.set_marks(&remotes, &camera(), None);
    d.set_entity_visibility(&remotes, &camera(), Some(&HalfWall), Some((5, 2)));
    assert_eq!(tags(&d), [(12, MarkState::Mate)]);
    // a "No-recoil only" room: the window drops the bridge after the wiring, nothing is drawn or forced
    let mut n = driver();
    mods.wire(&mut n);
    board(&mut n);
    n.xray_style = on;
    n.state.apply(&health_of(11, 50), 0, 0.0);
    if !crate::window::room_wallhacks(Some(dc_wire::roomhacks::NO_RECOIL)) {
        n.xray = None;
    }
    n.set_marks(&remotes, &camera(), None);
    n.set_entity_visibility(&remotes, &camera(), Some(&HalfWall), Some((5, 2)));
    assert!(n.state.xray.is_empty() && n.xray_draws.is_empty());
    assert_eq!(tags(&n), [(12, MarkState::Mate)]);
    drop(d);
    drop(n);
    drop(mods);
}

/// CLI-1631 (amended, project decision): a visible enemy's forced tag and bar vanish in the frame of his
/// death (the 0x6d dead flag, or a DSV-480 health of 0), with no grey dead tag waiting for the respawn, come back
/// with the respawn, and are gone for good when he leaves; the dead mate keeps the 2009 dead tag.
#[test]
fn a_dead_or_departed_enemy_loses_tag_and_bar_at_once_and_a_respawn_brings_them_back() {
    use crate::hud::marks::MarkState;
    let _g = INSTALL.lock().unwrap_or_else(|e| e.into_inner());
    let r = recipe(&[(&hacking_mod(), ModOrigin::Server)], false);
    let mods = crate::modassets::MatchMods::open(Some(&r)).expect("the demo mod loads");
    let remotes = [body(11, 1, 150.0), body(12, 2, -150.0)];
    let mut d = driver();
    mods.wire(&mut d);
    board(&mut d);
    d.xray_style = XrayStyle {
        nametags: true,
        health: true,
        ..XrayStyle::default()
    };
    d.state.apply(&health_of(11, 80), 0, 0.0);
    d.state.apply(&health_of(12, 80), 0, 0.0);
    let frame = |d: &mut HudDriver| {
        d.set_marks(&remotes, &camera(), None);
        d.set_entity_visibility(&remotes, &camera(), Some(&HalfWall), Some((5, 2)));
    };
    frame(&mut d);
    assert_eq!(tags(&d), [(11, MarkState::Enemy), (12, MarkState::Mate)]);
    assert_eq!(bars(&d), 2);
    // the kill (0x6d) of both: the enemy's tag and bar are gone at once; the mate keeps the 2009 dead tag, no bar
    d.state.apply(&kill(11), 10, 0.0);
    d.state.apply(&kill(12), 10, 0.0);
    frame(&mut d);
    assert_eq!(tags(&d), [(12, MarkState::Dead)]);
    assert_eq!(bars(&d), 0, "{:?}", d.state.xray);
    // the respawn (0x6b refresh, dead 0): both back
    for (pid, team) in [(11u8, 1u8), (12, 2)] {
        d.state.apply(
            &crate::hud::state::HudEvent::Roster(dc_botclient::shell::RosterRecord {
                name: format!("p{pid}"),
                pid,
                team,
                kind: 2,
                dead: false,
                complete: true,
                ..Default::default()
            }),
            20,
            0.0,
        );
    }
    frame(&mut d);
    assert_eq!(tags(&d), [(11, MarkState::Enemy), (12, MarkState::Mate)]);
    assert_eq!(bars(&d), 2);
    // DSV-480 health 0 (the server's "not alive") hides the enemy's tag and bar before any kill line
    d.state.apply(&health_of(11, 0), 30, 0.0);
    frame(&mut d);
    assert_eq!(tags(&d), [(12, MarkState::Mate)]);
    assert_eq!(bars(&d), 1);
    d.state.apply(&health_of(11, 100), 40, 0.0);
    frame(&mut d);
    assert_eq!(tags(&d).len(), 2);
    // the enemy leaves (0x6c): no tag, no bar, even while his body is still listed
    d.state.apply(&crate::hud::state::HudEvent::Left(11, "p11".into()), 50, 0.0);
    frame(&mut d);
    assert_eq!(tags(&d), [(12, MarkState::Mate)]);
    assert_eq!(bars(&d), 1);
    drop(d);
    drop(mods);
}

#[test]
fn nametags_and_health_commands_toggle_the_switches() {
    let _g = INSTALL.lock().unwrap_or_else(|e| e.into_inner());
    use crate::hud::chat::{ChatPress, LocalCommand};
    let typed = |d: &mut HudDriver, line: &str, want: LocalCommand| {
        d.chat_press(ChatPress::Enter);
        d.chat_press(ChatPress::Text(line.into()));
        let o = d.chat_press(ChatPress::Enter);
        assert_eq!(o.command, Some(want));
        assert!(o.send.is_none(), "never sent");
    };
    let last_line = |d: &HudDriver| d.state.chat.back().map(|l| l.text.clone());
    // no render bridge: nothing changes, the notice says why
    let mut plain = driver();
    plain.xray_style = XrayStyle::default();
    let _ = crate::settings::take_xray_note();
    typed(&mut plain, "/nametags", LocalCommand::Nametags);
    typed(&mut plain, "/health", LocalCommand::HealthBars);
    assert!(!plain.xray_style.nametags && !plain.xray_style.health);
    assert_eq!(
        last_line(&plain).as_deref(),
        Some(crate::hud::WALLHACK_NO_MOD)
    );
    assert_eq!(crate::settings::take_xray_note(), None);
    // a declared x-ray server: each command flips its switch, noted for the menu to save
    let root = temp("infocmd");
    let m = mod_dir(
        &root,
        "xr",
        "shared",
        r#""hooks.render", "world.full_visibility""#,
        "return function(mod) end",
    );
    let h = recipe(&[(&m, ModOrigin::Server)], false).load().unwrap();
    let mut d = driver();
    d.xray = LuaRenderHook::new(Rc::new(RefCell::new(h)));
    d.xray_style = XrayStyle::default();
    typed(&mut d, "/nametags", LocalCommand::Nametags);
    assert!(d.xray_style.nametags);
    assert_eq!(last_line(&d).as_deref(), Some("Nametags: on"));
    assert_eq!(
        crate::settings::take_xray_note().map(|s| s.nametags),
        Some(true)
    );
    typed(&mut d, "/health", LocalCommand::HealthBars);
    assert_eq!(last_line(&d).as_deref(), Some("Health bars: on"));
    typed(&mut d, "/healthbars", LocalCommand::HealthBars);
    assert!(!d.xray_style.health && d.xray_style.nametags);
    assert_eq!(last_line(&d).as_deref(), Some("Health bars: off"));
    assert_eq!(
        crate::settings::take_xray_note().map(|s| (s.nametags, s.health)),
        Some((true, false))
    );
    crate::settings::note_xray_style(XrayStyle::default());
    let _ = crate::settings::take_xray_note();
    drop(d);
    let _ = std::fs::remove_dir_all(&root);
}

/// The optional cosmetic mod of the repository.
fn drawn_crosshair() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/mods/drawn_crosshair")
}

fn crosshair_settings(colour: &str, style: &str, recoil: bool) -> std::collections::BTreeMap<String, std::collections::BTreeMap<String, String>> {
    let values = [
        ("colour".to_string(), colour.to_string()),
        ("style".to_string(), style.to_string()),
        ("recoil".to_string(), recoil.to_string()),
    ];
    [("drawn_crosshair".to_string(), values.into_iter().collect())]
        .into_iter()
        .collect()
}

/// A driver of the match's HUD at `w` x `h` with the mod wired, a rifle's green mark and a radar frame (the earlier
/// override bug showed only with one).
fn crosshair_driver(mods: &crate::modassets::MatchMods, w: u32, h: u32) -> HudDriver {
    let hud = match dc_rom::test_rom() {
        Some(rom) => Hud::from_rom(&rom, w, h, None).expect("ROM HUD"),
        None => Hud::fallback(w, h, None),
    };
    let mut d = HudDriver::new(hud);
    mods.wire(&mut d);
    d.state.life_state = 1;
    d.state.health = Some(100.0);
    d.state.own_pid = Some(0);
    d.state.now_ms = 5000;
    d.state.aim_mark = Some(crate::spread::AimMarkView {
        shape: crate::spread::mark::SHAPE_PLAIN,
        distance: 10.0,
        since_shot_s: None,
        colour: [0, 255, 0, 255],
        rest: 10.0,
    });
    d.state.radar = Some(crate::hud::radar::RadarFrame {
        own: [0.0; 3],
        yaw: 0.0,
        blips: Vec::new(),
    });
    d
}

fn the_drawn(items: &[crate::hud::DrawItem]) -> Vec<crate::hud::crosshair::DrawnCrosshair> {
    items
        .iter()
        .filter_map(|i| match i {
            crate::hud::DrawItem::Crosshair(c) => Some(*c),
            _ => None,
        })
        .collect()
}

/// CLI-1633: each style in the chosen colour through the real overlay, at both sizes, with a radar frame; a ring is
/// round on 16:9; no stock green. Pictures go where `DC_CROSSHAIR_SHOTS` says.
#[test]
fn each_style_draws_in_its_colour_on_the_gpu_layer_while_the_radar_draws() {
    use crate::world::GpuTarget;
    use crate::WorldSource;
    let _g = INSTALL.lock().unwrap_or_else(|e| e.into_inner());
    let ctx = match dc_render::GpuContext::headless(dc_render::AdapterChoice::Auto) {
        Ok(c) => c,
        Err(e) => {
            println!("skipped: no GPU adapter ({e})");
            return;
        }
    };
    let renderer = dc_render::WorldRenderer::new(
        &ctx,
        dc_render::OFFSCREEN_FORMAT,
        dc_render::RenderSettings::default(),
    );
    let world = crate::testkit::synth_source()
        .load(Some(GpuTarget {
            ctx: &ctx,
            renderer: &renderer,
        }))
        .expect("synthetic world");
    let camera = dc_render::Camera::looking(glam::Vec3::ZERO, 0.0, 0.0, 1.5, 1.2);
    let shots = std::env::var_os("DC_CROSSHAIR_SHOTS").map(PathBuf::from);
    for style in dc_mod_lua::CROSSHAIR_STYLES {
        for (w, h) in [(1280u32, 720u32), (2560, 1440)] {
            let target = crate::offscreen::Offscreen::new(&ctx, dc_render::OFFSCREEN_FORMAT, w, h);
            let mut r = recipe(&[(&drawn_crosshair(), ModOrigin::Local)], true);
            r.settings = crosshair_settings("#ff00ff", style, true);
            let mods = crate::modassets::MatchMods::open(Some(&r)).expect("the mod loads");
            let mut d = crosshair_driver(&mods, w, h);
            let mut layer = crate::overlay::OverlayRenderer::new(&ctx, dc_render::OFFSCREEN_FORMAT);
            let items = d.items_due(true).expect("a list");
            assert_eq!(the_drawn(&items).len(), 1, "{style}");
            layer.update_items(&ctx, &items, &mut d.hud.assets, (w, h));
            let frame = target
                .render_with_hud(
                    &ctx,
                    &renderer,
                    world.scene.as_ref().unwrap(),
                    &camera,
                    None,
                    &[],
                    &[],
                    Some(&layer),
                    None,
                )
                .unwrap();
            let box_half = 40 * w / 1280;
            let (cx, cy) = (w / 2 - 1, h / 2 - 1);
            let (mut magenta, mut green) = (0, 0);
            let (mut xs, mut ys) = ((u32::MAX, 0), (u32::MAX, 0));
            for y in cy - box_half..cy + box_half {
                for x in cx - box_half..cx + box_half {
                    let o = ((y * w + x) * 4) as usize;
                    let p = &frame.rgba[o..o + 4];
                    if p[0] > 200 && p[2] > 200 && p[1] < 60 {
                        magenta += 1;
                        xs = (xs.0.min(x), xs.1.max(x));
                        ys = (ys.0.min(y), ys.1.max(y));
                    }
                    if p[1] > 200 && p[0] < 60 && p[2] < 60 {
                        green += 1;
                    }
                }
            }
            assert!(magenta > 3, "{style} {w}x{h}: {magenta}");
            assert_eq!(green, 0, "{style} {w}x{h}: no stock green");
            assert_eq!(xs.1 - xs.0, ys.1 - ys.0, "{style} {w}x{h}: square extent on 16:9");
            if let (Some(dir), 2560) = (&shots, w) {
                let _ = std::fs::create_dir_all(dir);
                frame.save_png(&dir.join(format!("crosshair_{style}_{w}x{h}.png"))).unwrap();
            }
            drop(layer);
            drop(mods);
        }
    }
}

/// CLI-1633: recoil on follows the mark out and back; recoil off stays at rest; the hit flash still reaches it.
#[test]
fn the_drawn_ring_follows_the_mark_and_recoil_off_stays_fixed() {
    let _g = INSTALL.lock().unwrap_or_else(|e| e.into_inner());
    let open = |recoil: bool| {
        let mut r = recipe(&[(&drawn_crosshair(), ModOrigin::Local)], true);
        r.settings = crosshair_settings("#4080ff", "circle", recoil);
        crate::modassets::MatchMods::open(Some(&r)).expect("the mod loads")
    };
    let view = |distance: f32, since: Option<f32>| crate::spread::AimMarkView {
        shape: crate::spread::mark::SHAPE_PLAIN,
        distance,
        since_shot_s: since,
        colour: [0, 255, 0, 255],
        rest: 10.0,
    };
    let mods = open(true);
    let mut d = crosshair_driver(&mods, 1280, 720);
    let radius = |d: &mut HudDriver, v| {
        d.state.aim_mark = Some(v);
        let items = d.hud.items(&d.state);
        let c = the_drawn(&items);
        assert_eq!(c[0].colour, [0x40, 0x80, 0xff, 0xff]);
        c[0].ring_radius()
    };
    let rest = radius(&mut d, view(10.0, None));
    let shot = radius(&mut d, view(10.0, Some(0.0)));
    let wide = radius(&mut d, view(30.0, None));
    let back = radius(&mut d, view(10.0, None));
    assert_eq!(shot - rest, crate::spread::mark::FIRING_OFFSET);
    assert_eq!(wide - rest, 20.0);
    assert_eq!(back, rest);
    drop(d);
    drop(mods);
    let mods = open(false);
    let mut d = crosshair_driver(&mods, 1280, 720);
    let radius = |d: &mut HudDriver, v| {
        d.state.aim_mark = Some(v);
        let items = d.hud.items(&d.state);
        the_drawn(&items)[0].ring_radius()
    };
    let rest = radius(&mut d, view(10.0, None));
    assert_eq!(radius(&mut d, view(10.0, Some(0.0))), rest);
    assert_eq!(radius(&mut d, view(60.0, None)), rest);
    drop(d);
    drop(mods);
}

/// CLI-1633: a row with `DisableCrosshair` and a dead player draw nothing, as the stock mark.
#[test]
fn the_drawn_crosshair_keeps_the_marks_visibility_rules() {
    let _g = INSTALL.lock().unwrap_or_else(|e| e.into_inner());
    let r = recipe(&[(&drawn_crosshair(), ModOrigin::Local)], true);
    let mods = crate::modassets::MatchMods::open(Some(&r)).expect("the mod loads");
    let mut d = crosshair_driver(&mods, 1280, 720);
    assert_eq!(the_drawn(&d.hud.items(&d.state)).len(), 1);
    assert_eq!(mods.overrides.crosshair_colour, Some([0x40, 0x80, 0xff, 0xff]), "the default blue");
    d.state.aim_cross_hidden = true;
    assert!(the_drawn(&d.hud.items(&d.state)).is_empty(), "DisableCrosshair");
    d.state.aim_cross_hidden = false;
    d.state.between_rounds = true;
    assert!(the_drawn(&d.hud.items(&d.state)).is_empty(), "between rounds");
    drop(d);
    drop(mods);
}

// ---- CLI-1635: enemies in the player's picks, team mates in other colours or nothing

/// A driver on the demo mod's bridge with a board: the own player (pid 5) on `own_team`, p11 on team 1, p12 on team 2.
fn team_driver(mods: &crate::modassets::MatchMods, own_team: u8, class: u8) -> HudDriver {
    use crate::hud::state::BoardRow;
    let mut d = driver();
    mods.wire(&mut d);
    d.state.own_pid = Some(5);
    d.state.mode_class = Some(class);
    for (pid, team) in [(5, own_team), (11, 1), (12, 2)] {
        d.state.roster.insert(
            pid,
            BoardRow {
                pid,
                name: format!("p{pid}"),
                team,
                kills: 0,
                deaths: 0,
                dead: false,
                ping: None,
            },
        );
    }
    d.xray_style = XrayStyle {
        mode: XrayMode::Chams,
        primary: [0xff, 0xff, 0x00, 0xff],
        secondary: [0x00, 0xff, 0x00, 0xff],
        ..XrayStyle::default()
    };
    d
}

#[test]
fn the_enemy_takes_the_picks_on_either_team_and_the_mate_other_stable_colours() {
    let _g = INSTALL.lock().unwrap_or_else(|e| e.into_inner());
    let r = recipe(&[(&hacking_mod(), ModOrigin::Server)], false);
    let mods = crate::modassets::MatchMods::open(Some(&r)).expect("the demo mod loads");
    let remotes = [body(11, 1, 150.0), body(12, 2, -150.0)];
    for (own, enemy, mate) in [(1u8, 12u16, 11u16), (2, 11, 12)] {
        let mut d = team_driver(&mods, own, 1);
        let style = d.xray_style;
        d.set_entity_visibility(&remotes, &camera(), Some(&HalfWall), Some((5, own)));
        let of = |d: &HudDriver, id| *d.xray_draws.iter().find(|x| x.entity == id).unwrap();
        let (e, m) = (of(&d, enemy), of(&d, mate));
        assert_eq!((e.primary, e.secondary), (style.primary, style.secondary), "own team {own}");
        assert_ne!(m.primary, style.primary);
        assert_ne!(m.secondary, style.secondary);
        // stable from frame to frame and refresh to refresh
        for _ in 0..5 {
            d.state.now_ms += REFRESH_MS;
            d.set_entity_visibility(&remotes, &camera(), Some(&HalfWall), Some((5, own)));
            let again = of(&d, mate);
            assert_eq!((again.primary, again.secondary), (m.primary, m.secondary));
        }
    }
}

#[test]
fn teammates_off_draws_nothing_on_a_mate_and_the_mate_keeps_his_2009_tag() {
    use crate::hud::marks::MarkState;
    let _g = INSTALL.lock().unwrap_or_else(|e| e.into_inner());
    let r = recipe(&[(&hacking_mod(), ModOrigin::Server)], false);
    let mods = crate::modassets::MatchMods::open(Some(&r)).expect("the demo mod loads");
    let remotes = [body(11, 1, 150.0), body(12, 2, -150.0)];
    for mode in XrayMode::ALL {
        let mut d = team_driver(&mods, 2, 1);
        d.xray_style.mode = mode;
        d.xray_style.health = true;
        d.xray_style.nametags = true;
        d.xray_style.teammates = false;
        d.set_marks(&remotes, &camera(), None);
        d.set_entity_visibility(&remotes, &camera(), Some(&HalfWall), Some((5, 2)));
        assert!(d.xray_draws.iter().all(|x| x.entity == 11), "{mode:?}");
        let mate_rect = body_rect(&camera(), remotes[1].pos, remotes[1].half).unwrap();
        let on_mate = d.state.xray.iter().any(|i| match i {
            DrawItem::Fill { rect, .. } => {
                rect[0] + rect[2] / 2.0 >= mate_rect[0] - BAR_W
                    && rect[0] + rect[2] / 2.0 <= mate_rect[0] + mate_rect[2] + BAR_W
            }
            _ => false,
        });
        assert!(!on_mate, "{mode:?}: {:?}", d.state.xray);
        assert!(d
            .state
            .marks
            .iter()
            .any(|m| m.obj == 12 && m.state == MarkState::Mate));
    }
}

#[test]
fn in_a_free_for_all_everybody_is_an_enemy() {
    let _g = INSTALL.lock().unwrap_or_else(|e| e.into_inner());
    let r = recipe(&[(&hacking_mod(), ModOrigin::Server)], false);
    let mods = crate::modassets::MatchMods::open(Some(&r)).expect("the demo mod loads");
    let remotes = [body(11, 1, 150.0), body(12, 2, -150.0)];
    let mut d = team_driver(&mods, 1, 6);
    d.xray_style.teammates = false;
    let style = d.xray_style;
    d.set_entity_visibility(&remotes, &camera(), Some(&HalfWall), Some((5, 1)));
    assert_eq!(d.xray_draws.len(), 2);
    assert!(d.xray_draws.iter().all(|x| x.primary == style.primary));
    assert!(d.xray.as_ref().unwrap().ents.iter().filter(|e| !e.is_local).all(|e| e.is_enemy));
}

#[test]
fn mate_colours_are_never_close_to_any_pick_pair_and_follow_the_seed() {
    let palette = crate::menu::shopxray::PALETTE;
    let mut seen = std::collections::BTreeSet::new();
    for primary in palette {
        for secondary in palette {
            let style = XrayStyle {
                primary,
                secondary,
                ..XrayStyle::default()
            };
            for seed in [0u32, 1, 2, 7, 0x1234, 0xdead_beef, u32::MAX] {
                let [a, b] = mate_colours(&style, seed);
                assert_eq!([a, b], mate_colours(&style, seed), "the same seed, the same pair");
                assert_ne!(a, b);
                for c in [a, b] {
                    for pick in [primary, secondary] {
                        let d: i32 = (0..3).map(|i| (c[i] as i32 - pick[i] as i32).pow(2)).sum();
                        assert!(d >= 110 * 110, "{c:?} close to {pick:?}");
                    }
                }
                seen.insert(a);
            }
        }
    }
    assert!(seen.len() > 3, "the seed chooses: {seen:?}");
    assert_eq!(session_seed(), session_seed());
}

#[test]
fn the_hook_and_line_of_sight_refresh_at_the_fixed_rate_and_a_new_player_asks_at_once() {
    let _g = INSTALL.lock().unwrap_or_else(|e| e.into_inner());
    let r = recipe(&[(&hacking_mod(), ModOrigin::Server)], false);
    let mods = crate::modassets::MatchMods::open(Some(&r)).expect("the demo mod loads");
    let mut remotes = vec![body(11, 1, 150.0)];
    let mut d = team_driver(&mods, 2, 1);
    d.xray_style.mode = XrayMode::Box;
    d.state.now_ms = 1000;
    let frame = |d: &mut HudDriver, remotes: &[RemoteBody]| {
        d.set_entity_visibility(remotes, &camera(), Some(&HalfWall), Some((5, 2)));
        d.xray.as_ref().unwrap().calls
    };
    assert_eq!(frame(&mut d, &remotes), 1);
    // a thousand-frames-a-second burst inside the interval: the cached answer
    for ms in 1..REFRESH_MS {
        d.state.now_ms = 1000 + ms;
        assert_eq!(frame(&mut d, &remotes), 1);
    }
    // the box still follows the player between two calls
    let before = d.state.xray.clone();
    remotes[0].pos[0] += 20.0;
    assert_eq!(frame(&mut d, &remotes), 1);
    assert_ne!(d.state.xray, before, "drawn at the current place");
    d.state.now_ms = 1000 + REFRESH_MS;
    assert_eq!(frame(&mut d, &remotes), 2);
    // somebody joins: asked at once, no waiting for the interval
    remotes.push(body(12, 2, -150.0));
    assert_eq!(frame(&mut d, &remotes), 3);
    // the session clock wrapping does not stop it
    d.state.now_ms = u32::MAX;
    assert_eq!(frame(&mut d, &remotes), 4);
    d.state.now_ms = 40;
    assert_eq!(frame(&mut d, &remotes), 5);
}

#[test]
fn the_teammates_command_toggles_and_the_profile_keeps_it() {
    let _g = INSTALL.lock().unwrap_or_else(|e| e.into_inner());
    use crate::hud::chat::{ChatPress, LocalCommand};
    let typed = |d: &mut HudDriver| {
        d.chat_press(ChatPress::Enter);
        d.chat_press(ChatPress::Text("/teammates".into()));
        let o = d.chat_press(ChatPress::Enter);
        assert_eq!(o.command, Some(LocalCommand::Teammates));
        assert!(o.send.is_none(), "never sent");
    };
    let last_line = |d: &HudDriver| d.state.chat.back().map(|l| l.text.clone());
    let saved = crate::settings::xray_style();
    let _ = crate::settings::take_xray_note();
    // no bridge: nothing changes
    let mut plain = driver();
    plain.xray_style = XrayStyle::default();
    typed(&mut plain);
    assert!(plain.xray_style.teammates);
    assert_eq!(last_line(&plain).as_deref(), Some(crate::hud::WALLHACK_NO_MOD));
    assert_eq!(crate::settings::take_xray_note(), None);
    // a declared server: off, on, each noted for the menu to save
    let r = recipe(&[(&hacking_mod(), ModOrigin::Server)], false);
    let mods = crate::modassets::MatchMods::open(Some(&r)).expect("the demo mod loads");
    let mut d = driver();
    mods.wire(&mut d);
    d.xray_style = XrayStyle::default();
    for (on, line) in [(false, "Teammates: off"), (true, "Teammates: on")] {
        typed(&mut d);
        assert_eq!(d.xray_style.teammates, on);
        assert_eq!(last_line(&d).as_deref(), Some(line));
        assert_eq!(crate::settings::take_xray_note().map(|s| s.teammates), Some(on));
    }
    crate::settings::note_xray_style(saved);
    let _ = crate::settings::take_xray_note();
    // the default without the key is on; a malformed value keeps it
    let table = crate::settings::CommandTable { rows: Vec::new() };
    let names = |_: i32| String::new();
    let doc = crate::settings::ProfileDoc::parse("");
    assert!(crate::settings::Settings::from_doc(&doc, &table, &names).xray.teammates);
    let mut doc = crate::settings::ProfileDoc::parse("");
    doc.own_set(crate::settings::OWN_XRAY_TEAMMATES, "x");
    assert!(crate::settings::Settings::from_doc(&doc, &table, &names).xray.teammates);
}

#[test]
fn the_view_cull_keeps_everything_near_the_view_and_drops_what_is_far_outside() {
    use crate::characters::{ViewCull, CULL_RADIUS};
    let v = ViewCull::of(&camera());
    assert!(!v.outside([0.0, 0.0, 500.0]), "straight ahead");
    assert!(!v.outside([0.0, 0.0, -CULL_RADIUS + 10.0]), "just behind the eye: its sphere reaches the view");
    assert!(v.outside([0.0, 0.0, -2000.0]), "behind");
    assert!(v.outside([50000.0, 0.0, 500.0]) && v.outside([-50000.0, 0.0, 500.0]), "beside");
    assert!(v.outside([0.0, 50000.0, 500.0]) && v.outside([0.0, -50000.0, 500.0]), "above, below");
}

/// CLI-1633: the mod's settings through the real overlay: every option changes the frame's pixels, at both sizes, with
/// a radar frame in the HUD state.
#[test]
fn every_crosshair_option_changes_the_pixels_on_the_gpu_layer() {
    use crate::world::GpuTarget;
    use crate::WorldSource;
    let _g = INSTALL.lock().unwrap_or_else(|e| e.into_inner());
    let ctx = match dc_render::GpuContext::headless(dc_render::AdapterChoice::Auto) {
        Ok(c) => c,
        Err(e) => {
            println!("skipped: no GPU adapter ({e})");
            return;
        }
    };
    let renderer = dc_render::WorldRenderer::new(
        &ctx,
        dc_render::OFFSCREEN_FORMAT,
        dc_render::RenderSettings::default(),
    );
    let world = crate::testkit::synth_source()
        .load(Some(GpuTarget {
            ctx: &ctx,
            renderer: &renderer,
        }))
        .expect("synthetic world");
    let camera = dc_render::Camera::looking(glam::Vec3::ZERO, 0.0, 0.0, 1.5, 1.2);
    // a firing mark, so the recoil options show
    let firing = crate::spread::AimMarkView {
        shape: crate::spread::mark::SHAPE_PLAIN,
        distance: 30.0,
        since_shot_s: Some(0.02),
        colour: [0, 255, 0, 255],
        rest: 10.0,
    };
    let frame_of = |w: u32, h: u32, extra: &[(&str, &str)]| -> Vec<u8> {
        let target = crate::offscreen::Offscreen::new(&ctx, dc_render::OFFSCREEN_FORMAT, w, h);
        let mut r = recipe(&[(&drawn_crosshair(), ModOrigin::Local)], true);
        let mut v = crosshair_settings("#ff00ff", "cross", true);
        let m = v.get_mut("drawn_crosshair").unwrap();
        for (k, x) in extra {
            m.insert(k.to_string(), x.to_string());
        }
        r.settings = v;
        let mods = crate::modassets::MatchMods::open(Some(&r)).expect("the mod loads");
        let mut d = crosshair_driver(&mods, w, h);
        d.state.aim_mark = Some(firing);
        assert!(d.state.radar.is_some());
        let mut layer = crate::overlay::OverlayRenderer::new(&ctx, dc_render::OFFSCREEN_FORMAT);
        let items = d.items_due(true).expect("a list");
        assert_eq!(the_drawn(&items).len(), 1, "{extra:?}");
        layer.update_items(&ctx, &items, &mut d.hud.assets, (w, h));
        let f = target
            .render_with_hud(
                &ctx,
                &renderer,
                world.scene.as_ref().unwrap(),
                &camera,
                None,
                &[],
                &[],
                Some(&layer),
                None,
            )
            .unwrap();
        drop(layer);
        drop(mods);
        f.rgba
    };
    let options: &[&[(&str, &str)]] = &[
        &[("length", "15")],
        &[("thickness", "4")],
        &[("gap", "6")],
        &[("gap", "-6")],
        &[("dot", "true")],
        &[("t_style", "true")],
        &[("outline", "true")],
        &[("colour", "#ff00ff80")],
        &[("colour", "#00ffff")],
        &[("style", "circle")],
        &[("style", "circle_dot")],
        &[("style", "dot")],
        &[("style", "cross_dot")],
        &[("recoil", "false")],
        &[("recoil", "fire")],
        &[("recoil_scale", "2")],
    ];
    for (w, h) in [(1280u32, 720u32), (2560, 1440)] {
        let base = frame_of(w, h, &[]);
        for o in options {
            assert_ne!(frame_of(w, h, o), base, "{o:?} at {w}x{h}");
        }
        // the outline's width shows only with the outline on
        let on = frame_of(w, h, &[("outline", "true")]);
        assert_ne!(frame_of(w, h, &[("outline", "true"), ("outline_thickness", "3")]), on, "{w}x{h}");
        assert_eq!(frame_of(w, h, &[("outline_thickness", "3")]), base, "{w}x{h}: the outline is off");
    }
}

/// MOD-24: the values version 1.0.0 saved keep their meaning; a bad or out-of-range stored value reads as the default.
#[test]
fn saved_values_of_the_first_version_keep_working() {
    use crate::hud::crosshair::{CrossLook, CrosshairStyle, RecoilMode};
    let _g = INSTALL.lock().unwrap_or_else(|e| e.into_inner());
    let open = |m: Vec<(&str, &str)>| {
        let dir = drawn_crosshair();
        let decl = dc_mod_lua::ModManifest::from_file(&dir.join("manifest.json")).unwrap().settings;
        let stored = m.into_iter().map(|(a, b)| (a.to_string(), b.to_string())).collect();
        let values = dc_mod_lua::resolve_settings(&decl, Some(&stored));
        let mut r = recipe(&[(&dir, ModOrigin::Local)], true);
        r.settings = [("drawn_crosshair".to_string(), values)].into_iter().collect();
        let mods = crate::modassets::MatchMods::open(Some(&r)).expect("the mod loads");
        let ov = crate::modassets::hud_overrides(&mods.host.borrow()).unwrap();
        ov
    };
    let ov = open(vec![("colour", "#ff3030"), ("style", "cross_dot"), ("recoil", "false")]);
    assert_eq!(ov.crosshair_colour, Some([0xff, 0x30, 0x30, 0xff]));
    assert_eq!(ov.crosshair_style, Some(CrosshairStyle::CrossDot));
    assert_eq!(ov.cross_look(), CrossLook { recoil: RecoilMode::Static, ..CrossLook::default() });
    assert_eq!(open(vec![("recoil", "true")]).crosshair_recoil, Some(RecoilMode::Dynamic));
    let ov = open(vec![("length", "999"), ("gap", "1.3"), ("thickness", "wide"), ("style", "star"), ("recoil", "maybe")]);
    assert_eq!(ov.cross_look(), CrossLook::default());
    assert_eq!(ov.crosshair_style, Some(CrosshairStyle::Circle));
}

/// CLI-1636: `/help` lists the commands of this match, one line each, never sent; the mod's only with a bridge. Every
/// listed command and every command of the Hacking Mod's manifest is one the chat really takes.
#[test]
fn help_lists_the_local_commands_and_the_mods_only_with_a_bridge() {
    let _g = INSTALL.lock().unwrap_or_else(|e| e.into_inner());
    use crate::hud::chat::{local_command, ChatPress, LocalCommand, HELP_LINES, HELP_MOD_LINES};
    let help = |d: &mut HudDriver| -> Vec<String> {
        d.state.chat.clear();
        d.chat_press(ChatPress::Enter);
        d.chat_press(ChatPress::Text("/HELP".into()));
        let o = d.chat_press(ChatPress::Enter);
        assert_eq!(o.command, Some(LocalCommand::Help));
        assert!(o.send.is_none(), "never sent");
        d.state.chat.iter().map(|l| l.text.clone()).collect()
    };
    let mut plain = driver();
    let lines = help(&mut plain);
    assert_eq!(lines, HELP_LINES);
    assert!(lines.iter().any(|l| l.starts_with("/help")));
    assert!(lines.iter().any(|l| l.starts_with("/network (or /net)")));
    assert!(!lines.iter().any(|l| l.contains("/wallhack")), "{lines:?}");

    let root = temp("helpcmd");
    let m = mod_dir(&root, "xr", "shared", r#""hooks.render", "world.full_visibility""#, "return function(mod) end");
    let h = recipe(&[(&m, ModOrigin::Server)], false).load().unwrap();
    let mut d = driver();
    d.xray = LuaRenderHook::new(Rc::new(RefCell::new(h)));
    let lines = help(&mut d);
    let want: Vec<&str> = HELP_LINES.iter().chain(HELP_MOD_LINES).copied().collect();
    assert_eq!(lines, want, "all of them fit the chat window");
    assert_eq!(lines.len(), crate::hud::state::CHAT_MAX_LINES);
    for l in &want {
        let head = l.split_whitespace().next().unwrap();
        assert!(local_command(head).is_some(), "{l}");
    }
    let man = dc_mod_lua::ModManifest::from_file(&hacking_mod().join("manifest.json")).unwrap();
    assert_eq!(man.commands.len(), 4);
    for c in &man.commands {
        assert!(local_command(&c.command).is_some(), "{}", c.command);
        assert!(want.iter().any(|l| l.starts_with(&c.command)), "{}", c.command);
    }
}

/// use-items-sweep (CLI-1878): a reticle item in use draws the mark in its shape and colour on the composed frame;
/// without one the plain green mark stays.
#[test]
fn a_reticle_item_draws_its_shape_and_colour_in_the_frame() {
    let hud = match dc_rom::test_rom() {
        Some(rom) => Hud::from_rom(&rom, 1024, 768, None).expect("ROM HUD"),
        None => Hud::fallback(1024, 768, None),
    };
    let mut d = HudDriver::new(hud);
    d.state.life_state = 1;
    d.state.health = Some(100.0);
    d.state.own_pid = Some(0);
    d.state.now_ms = 5000;
    d.state.aim_mark = Some(crate::spread::AimMarkView {
        shape: crate::spread::mark::SHAPE_PLAIN,
        distance: 10.0,
        since_shot_s: None,
        colour: [0, 255, 0, 255],
        rest: 10.0,
    });
    let fills = |d: &mut HudDriver| -> Vec<([i32; 4], [f32; 4])> {
        let items = d.items_due(true).expect("a list");
        crate::hud::compose::resolve(&items, &mut d.hud.assets, 1024, 768)
            .into_iter()
            .filter_map(|p| match p {
                crate::hud::compose::Prim::Fill { rect, colour } => Some((rect, colour)),
                _ => None,
            })
            .collect()
    };
    let green = [0.0, 255.0, 0.0, 255.0];
    let plain: Vec<_> = fills(&mut d).into_iter().filter(|f| f.1 == green).collect();
    assert_eq!(plain.len(), 4, "the plain mark's four arms");
    d.set_reticle(crate::spread::mark::shape_of(4), 0xff12_3456);
    let mark: Vec<_> = fills(&mut d)
        .into_iter()
        .filter(|f| f.1 == [18.0, 52.0, 86.0, 255.0])
        .collect();
    assert!(mark.len() > 30, "the ring and the dot: {}", mark.len());
    assert!(
        mark.iter().any(|f| f.0 == [511, 383, 512, 384]),
        "the centre pixel"
    );
    assert!(fills(&mut d).iter().all(|f| f.1 != green));
    // a colour outside the table keeps the weapon's colour with the item's shape
    d.set_reticle(crate::spread::mark::shape_of(2), 0);
    let t: Vec<_> = fills(&mut d).into_iter().filter(|f| f.1 == green).collect();
    assert_eq!(t.len(), 4, "three arms and the dot");
}
