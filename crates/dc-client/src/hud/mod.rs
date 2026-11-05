//! The in-match HUD of the 2009 client, drawn from the user's ROM UI data.
//!
//! - [`state`]: what is shown, kept from the server's messages (0x66, 0x67, 0x6b, 0x6c, 0x6d, 0xb5, 0x44 / 0x45 /
//!   0x41, 0x46, 0x74) the way the 2009 client keeps it.
//! - [`layout`]: [`HudLayout`], every element's textures, places, fonts and colours from the HUD layout set
//!   (dc-ui-data's [`dc_ui_data::HudView`]), or our own text layout without a ROM.
//! - [`items`]: the display list of a frame and the [`HudDrawHook`] seam for mods.
//! - [`compose`]: the CPU compositor (ROM DTX textures, ROM TrueType fonts) into one RGBA image per frame, which
//!   [`crate::overlay::OverlayRenderer`] blends over the world.
//!
//! Elements: health / armour digits with their icons, stance and stamina gauge, the arcs beside the aim mark; clip and
//! reserve digits, fire mode, weapon-name image; kill feed with weapon and head-shot icons; the top team counters;
//! the round clock; the damage direction arcs; the respawn wait; join / leave notices in the chat log; the
//! scoreboard while its key is held; names over the players' heads and the team-mate panel of the aim cross
//! ([`marks`]).
// SPEC: FMT_CRES_UI.md §8 (HUD elements), GAME_MESSAGES_2009_S2C.md (see `state`), RENDER_2009.md §12

pub mod banners;
pub mod board;
pub mod bomb;
pub mod chat;
pub mod compose;
pub mod coopscore;
pub mod crosshair;
pub mod death;
pub mod exitbox;
pub mod fallborder;
pub mod feedmarks;
pub mod items;
pub mod layout;
pub mod letterbox;
pub mod marks;
pub mod mission;
pub mod modetop;
pub mod modhud;
pub mod msgs;
pub mod netview;
pub mod npcbar;
pub mod panel;
pub mod qrt;
pub mod radar;
pub mod rank;
pub mod scope;
pub mod spy;
pub mod state;
pub mod prompt;
pub mod teamchange;
pub mod vitals;
pub mod xray;

pub use compose::{compose, HudAssets, HudFonts, NoTextures, RomTextureSource, Tex, TextureSource};
pub use items::{build, DrawItem, HudDrawHook};
pub use layout::HudLayout;
pub use state::{AmmoView, BoardRow, CaseMark, FeedLine, HudState, HudTexts, Relation};

use crate::overlay::text::RgbaImage;
use dc_bute::ButeDoc;
use dc_ui_data::{
    FontResolver, HudKeys, HudLiteral, LayoutTable, SystemFonts, UiNames, UiStrings, VirtualScaler,
};
use std::collections::BTreeMap;
use std::path::Path;

/// Cell height of the default font when a style names no record (fallback layout), in virtual pixels.
// DC-DEFAULT: CLI-44 (proposed) part of the no-ROM layout
pub const FALLBACK_FONT_CELL: f32 = 18.0;

/// The HUD of one render target: layout, assets, optional mod hook, and the last composed image.
pub struct Hud {
    pub layout: HudLayout,
    pub assets: HudAssets,
    pub hook: Option<Box<dyn HudDrawHook>>,
    /// The HUD override keys of the loaded mod set (CLI-1613; empty without mods).
    pub overrides: crate::modassets::HudOverrides,
    /// CLI-1613: what the crosshair element held when it was last logged (`kind`, see [`modhud::crosshair_drawn`]).
    crosshair_logged: Option<String>,
    size: (u32, u32),
    /// Where the layout came from (logs).
    pub source: String,
    /// The radar (top right): the ROM layout, our own without a ROM, `None` when the ROM keys do not resolve.
    pub radar: Option<radar::Radar>,
    /// The own kill banners ([`banners`]).
    pub banners: banners::BannerLayout,
    /// The leave-the-match box: the shell's box textures and its texts ([`crate::loading::LeaveBox`]).
    pub leave_box: (crate::menu::popup::BoxArt, crate::menu::popup::BoxTexts),
    /// The exit box one Escape opens ([`exitbox`]).
    pub exit_box: exitbox::ExitBox,
    /// A display list the window draws over the HUD (the in-match report form, CLI-950), in virtual 1024 x 768
    /// coordinates like every HUD item.
    pub overlay: Vec<DrawItem>,
    /// CLI-1604: a software cursor drawn last (the iPad's match while the mouse is free: no system pointer there).
    pub cursor: Option<DrawItem>,
    /// The ROM `LetterBoxPercent` ([`letterbox`]).
    pub letterbox_percent: f32,
}

/// The ROM `LetterBoxPercent` (the client attribute file's camera section), `None` when it does not resolve.
// OBSERVED: ROM table key CLIENTBUTES.TXT [Camera] LetterBoxPercent
fn load_letterbox_percent(rom: &Path) -> Option<f32> {
    let vfs = dc_rez::RezVfs::new(rom);
    let doc = dc_bute::ButeDoc::from_vfs(&vfs, "ATTRIBUTES.rez", crate::sound::CLIENT_ENTRY)?;
    let v = doc
        .get(letterbox::PERCENT_SECTION)?
        .get(letterbox::PERCENT_KEY)?;
    v.trim()
        .trim_end_matches('f')
        .parse::<f32>()
        .ok()
        .filter(|v| v.is_finite())
}

/// The leave box's texts from the string table (`#id` without one, as the menu's ROM-less boxes).
// EVIDENCE: cshell+0x48e80 (title 0x371, text 0xbbf, buttons 0x35 / 0x36)
fn leave_box_texts(strings: Option<&UiStrings>) -> crate::menu::popup::BoxTexts {
    use crate::loading::{LEAVE_NO, LEAVE_TEXT, LEAVE_TITLE, LEAVE_YES};
    let t = |id: i32| match strings {
        // a missing string reads `#id` as in the menu's catalogue (`Catalog::text`)
        Some(s) => s
            .get(id as u32)
            .map_or_else(|| format!("#{id}"), str::to_string),
        None => format!("#{id}"),
    };
    crate::menu::popup::BoxTexts {
        title: t(LEAVE_TITLE),
        body: t(LEAVE_TEXT),
        captions: vec![t(LEAVE_YES), t(LEAVE_NO)],
    }
}

/// Kill-feed icon and weapon-name image of each weapon row (`[Weapon<n>]`), by the two row keys read from CShell.
pub fn weapon_hud_images(
    rom: &Path,
    keys: &HudKeys,
) -> BTreeMap<u16, (Option<String>, Option<String>)> {
    let vfs = dc_rez::RezVfs::new(rom);
    let Ok(Some(doc)) = ButeDoc::try_from_vfs(
        &vfs,
        crate::arms::ATTRIBUTES_REZ,
        crate::arms::WEAPONS_ENTRY,
    ) else {
        return BTreeMap::new();
    };
    let (icon_key, name_key) = (
        keys.literal(HudLiteral::WeaponKillIcon),
        keys.literal(HudLiteral::WeaponNameImage),
    );
    let mut out = BTreeMap::new();
    for (name, sec) in doc.iter() {
        // OBSERVED: ROM table key WEAPONS.TXT [Weapon<n>] (as `arms::WeaponTable`)
        let Some(n) = name
            .strip_prefix("Weapon")
            .filter(|r| !r.is_empty() && r.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|r| r.parse::<u16>().ok())
        else {
            continue;
        };
        let get = |k: &str| {
            sec.iter()
                .find(|(kk, _)| kk.eq_ignore_ascii_case(k))
                .map(|(_, v)| v.trim().trim_matches('"').to_string())
                .filter(|v| !v.is_empty())
        };
        out.insert(n, (get(icon_key), get(name_key)));
    }
    // VAR-1: a variant's kill-feed icon and name image are its base gun's
    for v in crate::camo::variants() {
        if let (Ok(row), Some(images)) = (
            u16::try_from(v.row),
            u16::try_from(v.base).ok().and_then(|b| out.get(&b).cloned()),
        ) {
            out.insert(row, images);
        }
    }
    out
}

/// The `(font, text)` pairs [`items::build`] measures: the compositor's atlases answer them before the display list
/// is built (its `measure` answers 0 for anything not listed here).
/// The ladder's gun names: the ROM shop's product name of each row, `#<row>` without one.
pub fn gun_ladder_names(rom: Option<&std::path::Path>, ladder: &[u16]) -> Vec<String> {
    let cat = rom.and_then(|r| dc_bute::ShopCatalog::from_rom(r).ok());
    let name = |w: u16| {
        let rows = cat.as_ref()?;
        let p = (0..0x10000).find_map(|pid| {
            let p = rows.product(pid)?;
            let row = rows.info(p.items.first()?.0)?;
            (row.itemid == i32::from(w) && p.view != 0).then_some(p)
        })?;
        let n = p.name.trim().trim_matches('"').trim().to_string();
        (!n.is_empty()).then_some(n)
    };
    ladder
        .iter()
        .map(|&w| name(w).unwrap_or_else(|| format!("#{w}")))
        .collect()
}

fn measured_texts(layout: &HudLayout, state: &HudState) -> Vec<(i32, String)> {
    let mut out = Vec::new();
    for l in &state.feed {
        for t in [&l.killer, &l.victim] {
            out.push((layout.feed.font, t.clone()));
        }
        out.push((layout.feed.font, format!(" [{}] ", l.weapon)));
    }
    // client-hudgaps: the caption widths place the rank icons of the world marks
    for m in &state.marks {
        out.push((layout.marks.style.font, m.name.clone()));
    }
    for l in &state.chat {
        for (_, seg) in items::markup_segments(&l.text, [0; 4]) {
            out.push((layout.chat.font, seg));
        }
    }
    // the caret of the open chat input sits after the typed line
    if state.chat_input.mode().is_some() {
        out.push((
            layout.chat_look.input_font,
            state.chat_input.text().to_string(),
        ));
    }
    if let Some(mk) = &layout.mk {
        for n in &state.mk {
            out.push((mk.font, n.name.clone()));
        }
    }
    out
}

impl Hud {
    /// The HUD from the user's ROM for a `width` x `height` target; `default_font` (the overlay font's bytes) serves
    /// records whose own file does not resolve.
    pub fn from_rom(
        rom: &Path,
        width: u32,
        height: u32,
        default_font: Option<(Vec<u8>, usize)>,
    ) -> Result<Hud, String> {
        let keys = HudKeys::from_rom(rom).map_err(|e| e.to_string())?;
        let names = UiNames::from_rom(rom).map_err(|e| e.to_string())?;
        let vfs = dc_rez::RezVfs::new(rom);
        let table = LayoutTable::load(&vfs, names).map_err(|e| e.to_string())?;
        let strings = UiStrings::from_rom(rom).ok();
        let rows = weapon_hud_images(rom, &keys);
        // the HUD set index is an engine console value (default 0); the 2009 ROM has set 0 only
        // SPEC: FMT_CRES_UI.md §8.2
        let mut layout = HudLayout::from_rom(&table, &keys, 0, strings.as_ref(), &rows);
        // the chat colours and input element (own pinned keys, same CShell and table)
        layout.chat_look =
            chat::ChatLook::from_rom(rom, &table).unwrap_or_else(chat::ChatLook::fallback);
        // the death panel (its own pinned keys, read from the same CShell and table)
        layout.death = death::DeathLayout::from_rom(rom, &table).unwrap_or_default();
        // the activate-type table of the door prompt (its own pinned literals, CLI-1230)
        layout.top.prompt.types =
            dc_ui_data::activate::ActivateTypes::from_rom(rom).unwrap_or_default();
        let radar = radar::RadarKeys::from_rom(rom)
            .ok()
            .and_then(|k| radar::RadarLayout::from_rom(&table, &k, 0))
            .map(radar::Radar::new);
        let mut kill_banners = banners::BannerLayout::from_view(
            &dc_ui_data::HudView::new(&table, &keys, 0),
            layout.opener_banner.as_ref(),
        );
        kill_banners.load_voices(rom);
        layout.low_health.beat_file = vitals::load_beat_file(rom);
        layout.board_sound_file = vitals::load_board_sound(rom);
        layout.rank_icons = rank::RankIcons::from_rom(rom);
        // client-scoreboard: the board's place / level / header columns (own pinned keys, same CShell and table)
        layout.board_columns =
            board::BoardColumns::from_rom(rom, &table, 0, strings.as_ref()).unwrap_or_default();
        let (panel_images, panel_classes) = panel::rom_rows(rom, &keys);
        layout.weapon_panel = panel::WeaponPanelLayout::from_view(
            &dc_ui_data::HudView::new(&table, &keys, 0),
            panel_images,
            panel_classes,
        );
        let fonts = SystemFonts::from_layout(&table);
        let resolver = FontResolver::open(rom).map_err(|e| e.to_string())?;
        let textures = RomTextureSource::open(rom)?;
        // CLI-658: the records that name a system face only are drawn with that installed face, as the menu draws
        // them (the ROM-file font was the board's wrong face); the caller's font and the ROM file serve without it
        let default = crate::menu::catalog::system_default(&fonts, &resolver)
            .map(|(_, bytes)| bytes)
            .or(default_font)
            .or_else(|| crate::overlay::load_rom_font_bytes(rom));
        let scale = VirtualScaler::new(width, height).sy;
        Ok(Hud {
            layout,
            assets: HudAssets::new(
                Box::new(textures),
                HudFonts::new(default, FALLBACK_FONT_CELL, Some((fonts, resolver)), scale),
            ),
            hook: None,
            overrides: Default::default(),
            crosshair_logged: None,
            size: (width, height),
            source: format!(
                "ROM HUD set 0 ({} weapon rows, {} kill icons, {} kill banners, {} banner voices)",
                rows.len(),
                rows.values().filter(|r| r.0.is_some()).count(),
                kill_banners.slots.len(),
                kill_banners.voices.len()
            ),
            radar,
            banners: kill_banners,
            leave_box: {
                let paths = crate::menu::catalog::CodePaths::from_rom(rom);
                (
                    crate::menu::popup::BoxArt {
                        window: paths.popup.clone(),
                        buttons: paths.popup_buttons.clone(),
                        warning: paths.warning.clone(),
                    },
                    leave_box_texts(strings.as_ref()),
                )
            },
            exit_box: exitbox::ExitBox::from_rom(rom, &table)
                .unwrap_or_else(exitbox::ExitBox::fallback),
            overlay: Vec::new(),
            cursor: None,
            letterbox_percent: load_letterbox_percent(rom).unwrap_or(letterbox::DEFAULT_PERCENT),
        })
    }

    /// Our own text layout with `font` (no ROM).
    pub fn fallback(width: u32, height: u32, font: Option<(Vec<u8>, usize)>) -> Hud {
        let scale = VirtualScaler::new(width, height).sy;
        Hud {
            layout: HudLayout::fallback(),
            assets: HudAssets::new(
                Box::new(NoTextures),
                HudFonts::new(font, FALLBACK_FONT_CELL, None, scale),
            ),
            hook: None,
            overrides: Default::default(),
            crosshair_logged: None,
            size: (width, height),
            source: "fallback layout (no ROM)".into(),
            radar: Some(radar::Radar::new(radar::RadarLayout::fallback())),
            banners: banners::BannerLayout::default(),
            leave_box: (Default::default(), leave_box_texts(None)),
            exit_box: exitbox::ExitBox::fallback(),
            overlay: Vec::new(),
            cursor: None,
            letterbox_percent: letterbox::DEFAULT_PERCENT,
        }
    }

    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// The target was resized.
    pub fn set_size(&mut self, width: u32, height: u32) {
        self.size = (width, height);
        self.assets
            .fonts
            .set_scale(VirtualScaler::new(width, height).sy);
    }

    /// The display list of `state` (after the mod hook).
    pub fn items(&mut self, state: &HudState) -> Vec<DrawItem> {
        let sx = VirtualScaler::new(self.size.0, self.size.1).sx;
        let fonts = &mut self.assets.fonts;
        let mut widths: BTreeMap<(i32, String), f32> = BTreeMap::new();
        for (f, t) in measured_texts(&self.layout, state) {
            let w = fonts.atlas(f).map(|a| a.measure(&t) / sx).unwrap_or(0.0);
            widths.insert((f, t), w);
        }
        let measure =
            |f: i32, t: &str| -> f32 { widths.get(&(f, t.to_string())).copied().unwrap_or(0.0) };
        // texture sizes of the images drawn at their own size (the intro text image)
        let mut sizes: BTreeMap<String, [f32; 2]> = BTreeMap::new();
        let mut wanted: Vec<String> = Vec::new();
        if let Some(i) = &state.intro {
            wanted.extend([i.head.clone(), i.body.clone()]);
        }
        wanted.extend(state.scope.drawn().into_iter().map(|o| o.sprite.clone()));
        wanted.extend(
            self.layout
                .top
                .prompt
                .image
                .as_ref()
                .filter(|_| state.use_target.is_some())
                .map(|i| i.tex.clone()),
        );
        wanted.extend(
            self.layout
                .top
                .prompt
                .item
                .as_ref()
                .filter(|_| state.item_target)
                .map(|i| i.tex.clone()),
        );
        // the Fire Team caption's picture, drawn whole into its place (CLI-1860)
        wanted.extend(
            state
                .mission
                .caption
                .as_ref()
                .filter(|c| !c.picture.is_empty())
                .map(|c| c.picture.clone()),
        );
        if let (Some(g), Some(goal)) = (&self.layout.top.goal, state.goal) {
            let v = goal.min(999);
            for d in [v / 100, (v / 10) % 10, v % 10] {
                wanted.push(format!("{}{d}.dtx", g.prefix));
            }
        }
        for t in wanted {
            if let Some(tex) = self.assets.texture(&t) {
                sizes.insert(t, [tex.width as f32, tex.height as f32]);
            }
        }
        let tex_size = |t: &str| sizes.get(t).copied();
        // the letterbox bars first: the 2009 interface frame draws them before the HUD (cshell+0x1b73be)
        let mut items = Vec::new();
        // CLI-1614: where each named element starts (the names a `hud.draw` hook sees)
        let mut marks: Vec<(&'static str, usize)> = vec![("scope", 0)];
        // the scope pictures are scene sprites: under the whole HUD (CLI-615)
        if state.result_screen.is_none() {
            scope::push_items(&mut items, &state.scope, self.size, &tex_size);
        }
        marks.push(("letterbox", items.len()));
        letterbox::push_bars(
            &mut items,
            self.size.1,
            VirtualScaler::new(self.size.0, self.size.1).sy,
            state.letterbox.progress(state.now_ms),
            self.letterbox_percent,
        );
        // CLI-1847: the fall's red screen-edge vignette, over the world and under every HUD element
        marks.push(("fall_border", items.len()));
        if state.result_screen.is_none() && death::live_widgets(state) {
            fallborder::push_items(&mut items, state.fall_border, self.size.0, self.size.1);
        }
        // CLI-1620: the x-ray boxes over the world, under every HUD element
        marks.push(("xray", items.len()));
        items.extend(state.xray.iter().cloned());
        let base = items.len();
        let mut inner = Vec::new();
        items.extend(items::build_marked(
            &self.layout,
            state,
            &measure,
            &tex_size,
            &mut inner,
        ));
        marks.extend(inner.into_iter().map(|(n, i)| (n, base + i)));
        marks.push(("weapon_panel", items.len()));
        if let Some(p) = &self.layout.weapon_panel {
            panel::draw(p, state, &mut items);
        }
        // the radar's items go to the FRONT of the list (drawn first, `Radar::push_items`): the marks taken so far move
        // up by their count and the radar's element is the first one (CLI-1613 amended 2026-11-01: the unshifted
        // marks pointed the crosshair element at radar items, so a mod crosshair missed the aim mark in a match)
        if let Some(r) = self.radar.as_mut() {
            let before = items.len();
            r.push_items(&mut items, state, &mut self.assets, self.size);
            let added = items.len() - before;
            if added > 0 {
                for m in marks.iter_mut() {
                    m.1 += added;
                }
                marks.insert(0, ("radar", 0));
            }
        }
        marks.push(("kill_banners", items.len()));
        self.banners.draw(state, &mut items);
        marks.push(("net_overlay", items.len()));
        // CLI-1574: the network overlay, top left, over the HUD and under the boxes
        if state.net_mode != netview::OFF && !state.net_lines.is_empty() {
            let st = netview::style(&self.layout);
            let widest = match self.assets.fonts.atlas_sized(st.font, st.size) {
                Some(a) => state
                    .net_lines
                    .iter()
                    .map(|l| a.measure(l) / sx)
                    .fold(0.0, f32::max),
                None => 0.0,
            };
            netview::push_items(&self.layout, state, widest, &mut items);
        }
        // CLI-1596: the "Show FPS" line, top right, in the network overlay's style
        marks.push(("fps", items.len()));
        if let Some(line) = &state.fps_line {
            items.push(DrawItem::Text {
                style: netview::style(&self.layout),
                pos: [
                    dc_ui_data::VIRTUAL_WIDTH as f32 - netview::ORIGIN[0],
                    netview::ORIGIN[1],
                ],
                text: line.clone(),
                right: true,
            });
        }
        // m36 / CLI-920: the exit box (one Escape) or the leave question over the HUD, in its hover / pressed state
        // EVIDENCE: cshell+0x274dd0 (the exit box), cshell+0x48e80 (the shell's message box with Yes / No)
        marks.push(("leave_box", items.len()));
        if let Some(view) = &state.leave_box {
            match view.page {
                crate::loading::LeavePage::Exit => self.exit_box.push_items(view, &mut items),
                crate::loading::LeavePage::Ask => {
                    let (art, texts) = &self.leave_box;
                    crate::menu::popup::box_items(
                        &mut self.assets,
                        art,
                        texts,
                        &crate::menu::popup::yes_no_buttons(),
                        None,
                        (view.hover.or(Some(0)), view.down),
                        self.size.0 as f32 / 1024.0,
                        &mut items,
                    );
                }
            }
        }
        marks.push(("overlay", items.len()));
        items.extend(self.overlay.iter().cloned());
        // CLI-1613 / CLI-1614: the mod set's override keys, then its `hud.draw` hook (no hook: no Lua call)
        let mut spans = modhud::spans_of(&marks, items.len());
        if !self.overrides.is_empty() {
            let vs = VirtualScaler::new(self.size.0, self.size.1);
            let k = if vs.sy > 0.0 { vs.sx / vs.sy } else { 1.0 };
            modhud::apply_overrides(
                &self.overrides,
                state,
                &mut self.assets,
                &mut items,
                &mut spans,
                k,
            );
            if let Some(k) = self.overrides.hud_scale {
                modhud::scale_items(&mut items, &spans, k);
            }
            // the live session says which unit draws the crosshair, once per change of kind (not per frame)
            if let Some((kind, line)) =
                modhud::crosshair_drawn(&items, self.overrides.crosshair.as_deref(), self.size)
            {
                if self.crosshair_logged.as_deref() != Some(kind.as_str()) {
                    println!("{line}");
                    self.crosshair_logged = Some(kind);
                }
            }
        }
        if let Some(h) = self.hook.as_mut() {
            h.draw_frame(state, &self.layout, self.size, &spans, &mut items);
        }
        if let Some(c) = &self.cursor {
            items.push(c.clone());
        }
        items
    }

    /// The composed HUD image of `state`.
    pub fn render(&mut self, state: &HudState) -> RgbaImage {
        let items = self.items(state);
        compose(&items, &mut self.assets, self.size.0, self.size.1)
    }
}

/// Height above a case at which its indicator is drawn (world units): the console value's default.
// SPEC: HISEC_2009.md §3.3 step 2 (`FlagOffset`, default 70 world units)
// DC-DEFAULT: CLI-81 (proposed) the default 70 (no console value in DeCombat)
pub const CASE_MARK_LIFT: f32 = 70.0;

/// A world point on the target in virtual pixels (1024 x 768), `None` behind the camera.
pub fn project(camera: &dc_render::Camera, p: [f32; 3]) -> Option<[f32; 2]> {
    let c = camera.view_projection() * glam::Vec4::new(p[0], p[1], p[2], 1.0);
    if c.w <= 1e-4 {
        return None;
    }
    let (nx, ny) = (c.x / c.w, c.y / c.w);
    // NDC to the virtual screen (y down)
    Some([
        (nx + 1.0) * 0.5 * dc_ui_data::VIRTUAL_WIDTH as f32,
        (1.0 - ny) * 0.5 * dc_ui_data::VIRTUAL_HEIGHT as f32,
    ])
}

/// CLI-1625: the notice of `/wallhack` on a server without an x-ray mod.
/// CLI-1636: how long the `/help` lines stay (long enough to read six of them).
pub const HELP_LINE_MS: u32 = 15_000;
pub const WALLHACK_NO_MOD: &str = "no x-ray mod on this server";

/// Which HUD a frame loop draws.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum HudChoice {
    /// No HUD (the debug panel only).
    #[default]
    Off,
    /// Our text layout (no ROM needed).
    Fallback,
    /// The 2009 layout from the ROM at this folder (falls back to [`HudChoice::Fallback`] when it cannot load).
    Rom(std::path::PathBuf),
}

/// Recompose at least this often while nothing else changes (the clock, the decaying damage arcs).
// DC-DEFAULT: CLI-55 (proposed) HUD recomposed on every input and at most 20 times a second otherwise
pub const RECOMPOSE_MS: u32 = 50;

/// The HUD of a frame loop: state, layout, and when to recompose.
pub struct HudDriver {
    pub hud: Hud,
    pub state: HudState,
    last_compose: Option<u32>,
    dirty: bool,
    pub events_applied: u64,
    pub composed: u64,
    /// CLI-1620: the `render.entity_visibility` bridge of a declared x-ray server (`None`: no call per frame).
    pub xray: Option<xray::LuaRenderHook>,
    /// The spare buffer of the x-ray boxes (swapped with `state.xray`).
    xray_items: Vec<DrawItem>,
    /// CLI-1624: the player's x-ray style (the published setting at the HUD's start; `/wallhack` cycles its mode).
    pub xray_style: crate::settings::XrayStyle,
    /// CLI-1624: this frame's chams / skeleton players for the renderer (empty without a bridge).
    pub xray_draws: Vec<xray::XrayDraw>,
    /// CLI-1635: the seed of the team mates' colours and the colours of the last refresh.
    pub xray_seed: u32,
    xray_mates: [[u8; 4]; 2],
    /// CLI-1631: the aim target and posed name points of this frame's marks, kept so the x-ray call can redo the
    /// marks with its fresh answer (only filled while a bridge exists).
    mark_target: Option<u16>,
    mark_points: std::collections::BTreeMap<u16, [f32; 3]>,
}

impl HudDriver {
    pub fn new(hud: Hud) -> Self {
        let c = &hud.layout.chat;
        let state = HudState {
            texts: HudTexts {
                join: c.join_form.clone(),
                leave: c.leave_form.clone(),
                case_drop: c.case_drop_form.clone(),
                case_taken: c.case_taken_form.clone(),
                revenge: c.revenge_form.clone(),
                kicked: c.kicked_form.clone(),
                same_line: c.same_line_form.clone(),
                sensitivity: c.sensitivity_form.clone(),
                team_chat: c.team_chat_name.clone(),
            },
            chat_moderator_tag: hud.layout.chat_look.moderator_tag.clone(),
            ..HudState::default()
        };
        HudDriver {
            hud,
            state,
            last_compose: None,
            dirty: true,
            events_applied: 0,
            composed: 0,
            xray: None,
            xray_items: Vec::new(),
            xray_style: crate::settings::xray_style(),
            xray_draws: Vec::new(),
            xray_seed: xray::session_seed(),
            xray_mates: [[0; 4]; 2],
            mark_target: None,
            mark_points: std::collections::BTreeMap::new(),
        }
    }

    /// Builds the HUD a loop asked for; `default_font` serves records without a usable file and the fallback.
    pub fn open(
        choice: &HudChoice,
        width: u32,
        height: u32,
        default_font: Option<(Vec<u8>, usize)>,
    ) -> Option<(HudDriver, String)> {
        match choice {
            HudChoice::Off => None,
            HudChoice::Fallback => {
                let h = Hud::fallback(width, height, default_font);
                let s = h.source.clone();
                Some((HudDriver::new(h), s))
            }
            HudChoice::Rom(rom) => match Hud::from_rom(rom, width, height, default_font.clone()) {
                Ok(h) => {
                    let s = h.source.clone();
                    Some((HudDriver::new(h), s))
                }
                Err(e) => {
                    let h = Hud::fallback(width, height, default_font);
                    Some((
                        HudDriver::new(h),
                        format!("ROM HUD not loaded ({e}); fallback layout"),
                    ))
                }
            },
        }
    }

    /// Applies the frame's inputs (`local_ms`: the loop's own clock, used when the source has none).
    pub fn update(
        &mut self,
        fs: &crate::world::FrameState,
        local_ms: u32,
        view_yaw: f32,
        crouched: bool,
        show_board: bool,
    ) {
        let now = fs.clock_ms.unwrap_or(local_ms);
        for (t, e) in &fs.hud_events {
            self.state.apply(e, *t, view_yaw);
            self.events_applied += 1;
            self.dirty = true;
        }
        if self.state.ammo != fs.ammo
            || self.state.aim_mark != fs.aim_mark
            || self.state.scope != fs.scope
            || self.state.aim_cross_hidden != fs.aim_cross_hidden
            || self.state.aim_picture != fs.aim_picture
            || self.state.crouched != crouched
            || self.state.show_board != show_board
        {
            self.dirty = true;
        }
        // CLI-610: the letterbox follows the live camera's kind byte; while its bars move every frame recomposes
        let lb = fs
            .cam_shot
            .as_ref()
            .is_some_and(crate::camshot::CamShotView::letterbox);
        let (_, h) = self.hud.size();
        let pct = self.hud.letterbox_percent;
        let before = letterbox::bar_px(h, self.state.letterbox.progress(self.state.now_ms), pct);
        self.state.letterbox.set(lb, now);
        if letterbox::bar_px(h, self.state.letterbox.progress(now), pct) != before {
            self.dirty = true;
        }
        self.state.ammo = fs.ammo;
        self.state.aim_mark = fs.aim_mark;
        self.state.scope = fs.scope.clone();
        self.state.aim_cross_hidden = fs.aim_cross_hidden;
        self.state.aim_picture = fs.aim_picture.clone();
        self.state.crouched = crouched;
        // the board's open sound: the key going down outside the result board (the close is silent)
        if show_board && !self.state.show_board && self.state.result_mark.is_none() {
            self.state.board_opens += 1;
        }
        self.state.show_board = show_board;
        let life = self.hud.layout.feed.lifetime_ms.max(1);
        self.state.tick(now, life);
        vitals::tick(&mut self.state, &self.hud.layout.low_health, now);
        if let Some(mk) = &self.hud.layout.mk {
            self.state.expire_notes(now, mk.lifetime_ms.max(1));
        }
    }

    /// The own sprint gauge `(value, max)` of the frame (`None`: no gauge rule, drawn full): the stamina bar shows
    /// `trunc(value / max * 100)` percent.
    // EVIDENCE: cshell+0x95850 (the gauge element: value / max * 100.0 truncated, cshell+0x95b02..0x95b19, into the
    // bar control of range 0..100 built by cshell+0x975f0)
    pub fn set_stamina(&mut self, level: Option<(f32, i32)>) {
        let pct = level.and_then(|(v, max)| {
            (max > 0 && v.is_finite()).then(|| ((v / max as f32) * 100.0).clamp(0.0, 100.0) as u32)
        });
        if self.state.stamina_pct != pct {
            self.state.stamina_pct = pct;
            self.dirty = true;
        }
    }

    /// The network overlay's mode (CLI-1570); off clears its lines.
    pub fn set_net_mode(&mut self, mode: u8) {
        self.state.net_mode = mode.min(netview::PLAYERS);
        if self.state.net_mode == netview::OFF {
            self.state.net_lines.clear();
        }
        self.dirty = true;
    }

    /// The network overlay's lines of this refresh ([`netview::lines`]).
    pub fn set_net_lines(&mut self, lines: Vec<String>) {
        if lines != self.state.net_lines {
            self.state.net_lines = lines;
            self.dirty = true;
        }
    }

    /// CLI-1596: the "Show FPS" line (`None`: off); a changed text redraws the HUD.
    /// The player the death camera follows and his object (`None`: the own view or the own body): the death
    /// panel shows his record and health (CLI-128).
    pub fn set_observed(&mut self, target: Option<(u8, u16)>) {
        if target != self.state.observed {
            self.state.observed = target;
            self.dirty = true;
        }
    }

    pub fn set_fps_line(&mut self, line: Option<String>) {
        if line != self.state.fps_line {
            self.state.fps_line = line;
            self.dirty = true;
        }
    }

    /// CLI-1031: the sensitivity hotkeys' line at the HUD's time.
    pub fn sensitivity_changed(&mut self, v: i32) {
        let now = self.state.now_ms;
        self.state.sensitivity_changed(v, now);
        self.dirty = true;
    }

    /// A key press for the chat input at the HUD's time; the caller sends [`chat::ChatOutcome::send`] and releases
    /// the game's held keys when the box opened.
    pub fn chat_press(&mut self, p: chat::ChatPress) -> chat::ChatOutcome {
        let now = self.state.now_ms;
        let o = self.state.chat_input.press(p, now);
        if o.refused {
            self.state.chat_refused(now);
        }
        // CLI-1570: `/network` changes the overlay here and is never sent; the choice is kept for the next matches
        if let Some(chat::LocalCommand::NetOverlay(arg)) = o.command {
            let mode = netview::next_mode(self.state.net_mode, arg);
            if mode != self.state.net_mode {
                self.set_net_mode(mode);
                crate::settings::note_net_overlay(mode);
            }
        }
        if o.command == Some(chat::LocalCommand::Help) {
            self.show_help();
        }
        if o.command == Some(chat::LocalCommand::Wallhack) {
            self.cycle_wallhack();
        }
        if o.command == Some(chat::LocalCommand::Nametags) {
            self.toggle_xray_info(false);
        }
        if o.command == Some(chat::LocalCommand::HealthBars) {
            self.toggle_xray_info(true);
        }
        if o.command == Some(chat::LocalCommand::Teammates) {
            self.toggle_xray_teammates();
        }
        // CLI-1841: `/report` sends a diagnostic report now (packed and sent on a worker thread)
        if o.command == Some(chat::LocalCommand::Report) {
            crate::diag::send(crate::diag::Failure::new(
                crate::diag::Kind::Manual,
                "sent by hand with /report in a match",
            ));
            self.state.line_for(
                "Sending a diagnostic report to the server's owner...".to_string(),
                state::ChatKind::Notice,
                now,
                HELP_LINE_MS,
            );
        }
        if o.consumed {
            self.dirty = true;
        }
        o
    }

    /// CLI-1636: `/help`: the local commands of this match in the chat window; the mod's only with a bridge.
    // DC-DEFAULT: CLI-1636 (proposed, USER REQUEST 2026-11-01)
    pub fn show_help(&mut self) {
        let now = self.state.now_ms;
        let extra = if self.xray.is_some() { chat::HELP_MOD_LINES } else { &[] };
        for l in chat::HELP_LINES.iter().chain(extra) {
            self.state.line_for(l.to_string(), state::ChatKind::Notice, now, HELP_LINE_MS);
        }
        self.dirty = true;
    }

    /// CLI-1625: `/wallhack`: on a declared x-ray server (a render bridge exists) the next x-ray mode (box, chams,
    /// skeleton, box), in effect at once and saved by the menu, with a notice line `Wallhack: <mode>`; elsewhere
    /// nothing changes and the notice says so.
    // DC-DEFAULT: CLI-1625 (proposed)
    pub fn cycle_wallhack(&mut self) {
        let now = self.state.now_ms;
        if self.xray.is_none() {
            self.state
                .notice(WALLHACK_NO_MOD.to_string(), now);
        } else {
            self.xray_style.mode = self.xray_style.mode.next();
            crate::settings::note_xray_style(self.xray_style);
            self.state
                .notice(format!("Wallhack: {}", self.xray_style.mode.name()), now);
        }
        self.dirty = true;
    }

    /// CLI-1630: `/nametags` (`health` false) or `/health` (`health` true): on a declared x-ray server the x-ray's
    /// name tags or health bars switch on / off, in effect at once and saved by the menu, with a notice line
    /// `Nametags: on` / `Health bars: off`; elsewhere nothing changes and the notice says so.
    // DC-DEFAULT: CLI-1630 (proposed, USER REQUEST 2026-11-01)
    pub fn toggle_xray_info(&mut self, health: bool) {
        let now = self.state.now_ms;
        if self.xray.is_none() {
            self.state.notice(WALLHACK_NO_MOD.to_string(), now);
        } else {
            let (flag, name) = if health {
                (&mut self.xray_style.health, "Health bars")
            } else {
                (&mut self.xray_style.nametags, "Nametags")
            };
            *flag = !*flag;
            let on = if *flag { "on" } else { "off" };
            crate::settings::note_xray_style(self.xray_style);
            self.state.notice(format!("{name}: {on}"), now);
        }
        self.dirty = true;
    }

    /// CLI-1635: `/teammates`: on a declared x-ray server the team mates' x-ray switches on / off, in effect at once and
    /// saved by the menu, with a notice line `Teammates: on / off`; elsewhere nothing changes and the notice says so.
    // DC-DEFAULT: CLI-1635 (proposed, USER REQUEST 2026-11-01)
    pub fn toggle_xray_teammates(&mut self) {
        let now = self.state.now_ms;
        if self.xray.is_none() {
            self.state.notice(WALLHACK_NO_MOD.to_string(), now);
        } else {
            self.xray_style.teammates = !self.xray_style.teammates;
            let on = if self.xray_style.teammates { "on" } else { "off" };
            crate::settings::note_xray_style(self.xray_style);
            self.state.notice(format!("Teammates: {on}"), now);
        }
        self.dirty = true;
    }

    /// This frame's HiSec cases on screen: each case of `cases` (key, world position, countdown) projected with
    /// `camera` onto the target, raised by the indicator offset first.
    // SPEC: HISEC_2009.md §3.3 step 2 (the indicator point is the object's position raised by the console value of
    // the flag offset, default 70 world units, cs+0x19fde0; not drawn behind the camera)
    pub fn set_cases(&mut self, cases: &[crate::world::HudCase], camera: &dc_render::Camera) {
        let marks: Vec<CaseMark> = cases
            .iter()
            .map(|c| CaseMark {
                key: c.key,
                at: project(camera, [c.pos[0], c.pos[1] + CASE_MARK_LIFT, c.pos[2]]),
                remaining: c.remaining.max(0),
            })
            .collect();
        if marks != self.state.case_marks {
            self.dirty = true;
        }
        self.state.case_marks = marks;
    }

    /// This frame's capture flags and capture zones on screen (CLI-1073): each projected with `camera` from its point
    /// raised by the flag offset.
    // EVIDENCE: cshell+0x86f10
    pub fn set_flags(&mut self, flags: &[crate::world::HudFlag], camera: &dc_render::Camera) {
        let marks: Vec<modetop::FlagMark> = flags
            .iter()
            .map(|f| modetop::FlagMark {
                team: f.team,
                base: f.base,
                // a flag's point is raised by the flag offset, a zone's is not (cshell+0x86f10)
                at: project(
                    camera,
                    [
                        f.pos[0],
                        f.pos[1] + if f.base { 0.0 } else { modetop::FLAG_MARK_LIFT },
                        f.pos[2],
                    ],
                ),
                remaining: f.remaining,
                metres: modetop::metres_between(camera.pos.to_array(), f.pos),
            })
            .collect();
        if marks != self.state.flag_marks {
            self.dirty = true;
        }
        self.state.flag_marks = marks;
    }

    /// This frame's names over the players' heads and the team-mate panel: the object under the aim cross is found
    /// with `camera`'s forward ray against the remote boxes, stopped by `world`; the marks are projected with `camera`
    /// from the box points ([`marks::mark_point`]; [`HudDriver::set_marks_at`] with the posed name sockets).
    pub fn set_marks(
        &mut self,
        remotes: &[crate::world::RemoteBody],
        camera: &dc_render::Camera,
        world: Option<&dyn dc_sim::Collider>,
    ) {
        self.set_marks_at(remotes, camera, world, &std::collections::BTreeMap::new());
    }

    /// [`HudDriver::set_marks`] with this frame's posed name-socket points by object (the model scene's
    /// [`crate::characters::ModelScene::name_points`], taken after the frame's characters were posed); a remote
    /// without one keeps its box point.
    // EVIDENCE: cshell+0x89310 (the caption point is the posed socket, see marks::NAME_SOCKET)
    pub fn set_marks_at(
        &mut self,
        remotes: &[crate::world::RemoteBody],
        camera: &dc_render::Camera,
        world: Option<&dyn dc_sim::Collider>,
        points: &std::collections::BTreeMap<u16, [f32; 3]>,
    ) {
        let f = camera.forward();
        let target = marks::aim_target(remotes, camera.pos.to_array(), f.to_array(), world);
        // CLI-1635: the frames between two x-ray refreshes keep these marks, so only a refresh needs the inputs again
        let now = self.state.now_ms;
        if self.xray.as_ref().is_some_and(|x| x.due(now, remotes.len())) {
            self.mark_target = target;
            self.mark_points.clone_from(points);
        }
        let forced = self.forced_tags();
        let shown = marks::frame_marks_forced(
            &self.state,
            remotes,
            &|p| project(camera, p),
            target,
            points,
            &|obj| forced.contains(&obj),
        );
        let mate = marks::mate_target(&self.state, remotes, target);
        // CLI-1871: the AI health bars; CLI-1872: a live points popup recomposes every frame while it rises
        let bars = npcbar::frame_bars(
            &self.state,
            remotes,
            &|p| project(camera, p),
            target,
            points,
        );
        if bars != self.state.npc_bars
            || self
                .state
                .points_popup
                .is_some_and(|p| now.saturating_sub(p.at) <= npcbar::POPUP_MS)
            || self.state.coop.moving(now, &self.hud.layout.top.coop)
        {
            self.dirty = true;
        }
        self.state.npc_bars = bars;
        let flashing = !self.state.mark_hits.is_empty();
        if marks::step_hits(&mut self.state, target, now, &self.hud.layout.marks) || flashing {
            self.dirty = true;
        }
        if shown != self.state.marks || mate != self.state.mate_target {
            self.dirty = true;
        }
        self.state.marks = shown;
        self.state.mate_target = mate;
    }

    /// CLI-1631 (amended): the objects whose 2009 enemy tag the x-ray forces on: every player the hook's last answer
    /// marks `visible` while the name-tag switch is on (empty without a bridge).
    fn forced_tags(&self) -> Vec<u16> {
        match &self.xray {
            Some(x) if self.xray_style.nametags => {
                x.looks.iter().filter(|l| l.visible).map(|l| l.id).collect()
            }
            _ => Vec::new(),
        }
    }

    /// CLI-1620: the `render.entity_visibility` call (no bridge: nothing, no Lua) and the x-ray boxes it asks for. `own`
    /// = the own object and team, `world` answers each player's line of sight from the camera. CLI-1635: the line of
    /// sight, the Lua call and the forced tags refresh [`xray::REFRESH_MS`] apart; the frames between draw the last
    /// answer at the players' current places.
    pub fn set_entity_visibility(
        &mut self,
        remotes: &[crate::world::RemoteBody],
        camera: &dc_render::Camera,
        world: Option<&dyn dc_sim::Collider>,
        own: Option<(u16, u8)>,
    ) {
        let Some(x) = self.xray.as_mut() else {
            return;
        };
        self.xray_draws.clear();
        let mut items = std::mem::take(&mut self.xray_items);
        items.clear();
        let now = self.state.now_ms;
        let refresh = x.due(now, remotes.len());
        if refresh {
            let state = &self.state;
            xray::entities(
                remotes,
                camera.pos.to_array(),
                own,
                world,
                &|r| !marks::is_mate(state, r),
                &mut x.ents,
            );
            x.run();
            x.note_refresh(now, remotes.len());
            self.xray_mates = xray::mate_colours(&self.xray_style, self.xray_seed);
        }
        let rect_of = |id: u16| {
            remotes
                .iter()
                .find(|r| r.obj == id)
                .and_then(|r| xray::body_rect(camera, r.pos, r.half))
        };
        xray::push_items(
            &x.looks,
            &x.ents,
            &self.xray_style,
            self.xray_mates,
            &rect_of,
            &mut items,
            &mut self.xray_draws,
        );
        // CLI-1631 (amended): the 2009 enemy tags of the players the hook shows, redone with this frame's answer
        // (set_marks_at ran with the previous one), then their health bars above the tags
        if refresh {
            let forced = self.forced_tags();
            let shown = marks::frame_marks_forced(
                &self.state,
                remotes,
                &|p| project(camera, p),
                self.mark_target,
                &self.mark_points,
                &|obj| forced.contains(&obj),
            );
            if shown != self.state.marks {
                self.state.marks = shown;
                self.dirty = true;
            }
        }
        let Some(x) = self.xray.as_ref() else {
            return;
        };
        let state = &self.state;
        let info_of = |id: u16| {
            let r = remotes.iter().find(|r| r.obj == id)?;
            // a player who left has no board row
            state.roster.get(&r.pid)?;
            Some(xray::PlayerInfo {
                health: state.remote_health.get(&r.pid).copied(),
                tag_at: state.marks.iter().find(|m| m.obj == id).map(|m| m.at),
                alive: !marks::not_alive(state, r.pid),
            })
        };
        xray::push_info(
            &x.looks,
            &x.ents,
            &self.xray_style,
            self.xray_mates,
            self.hud.layout.marks.style,
            &rect_of,
            &info_of,
            &mut items,
        );
        if items != self.state.xray {
            std::mem::swap(&mut self.state.xray, &mut items);
            self.dirty = true;
        }
        self.xray_items = items;
    }

    /// This frame's radar: the camera's point and view, the entries of `remotes` (team mates within the radius).
    /// The radar alone does not make the HUD due: it follows the regular recompose ([`RECOMPOSE_MS`]).
    // DC-DEFAULT: CLI-144 (proposed) the radar is updated with the HUD image (at least every RECOMPOSE_MS), not every
    // frame as the 2009 element is
    pub fn set_radar(&mut self, remotes: &[crate::world::RemoteBody], camera: &dc_render::Camera) {
        let Some(l) = self.hud.radar.as_ref().map(|r| &r.layout) else {
            return;
        };
        let f = camera.forward();
        let yaw = f.x.atan2(f.z);
        self.state.radar = Some(radar::frame(
            l,
            &self.state,
            remotes,
            camera.pos.to_array(),
            yaw,
        ));
    }

    /// The "Reality" option: the damage-direction arcs light only while it is on (2009: off by default).
    // EVIDENCE: cshell+0x1f7d20 (the sector update sits inside the Reality gate); DC-DEFAULT: CLI-313 (proposed)
    /// CLI-1639: this match is Gun Game, these are its guns' names, rung 0 first.
    pub fn set_gun_game(&mut self, names: Vec<String>) {
        self.state.gun_game = Some(state::GunGameHud::new(names));
    }

    /// use-items-sweep: the reticle item in use (shape bits, `0xAARRGGBB`; 0 keeps the weapon's colour; CLI-1878).
    pub fn set_reticle(&mut self, shape: u16, argb: u32) {
        self.state.reticle = Some((shape, crate::menu::catalog::argb(argb)));
    }

    pub fn set_reality(&mut self, on: bool) {
        self.state.reality = on;
    }

    /// Whether a drawn round (0x46 with no winner) plays a draw voice; off, as 2009 plays none.
    // DC-DEFAULT: CLI-410 (staged) off
    pub fn set_draw_voice(&mut self, on: bool) {
        self.state.draw_voice = on;
    }

    /// The voice lines the HUD started since the last call (kill banners, round start and result, HiSec case), as sound cues for this frame.
    pub fn take_sound_cues(&mut self) -> Vec<crate::sound::SoundCue> {
        // with a ROM table, a line without a file is silent (the 2009 lookup finds nothing to play); without one
        // every line is still logged
        let banners = &self.hud.banners;
        let rom = !banners.voices.is_empty();
        let mut cues: Vec<crate::sound::SoundCue> = std::mem::take(&mut self.state.banner.voices)
            .into_iter()
            .filter(|v| !rom || banners.voice_file(*v).is_some())
            .map(|v| crate::sound::SoundCue::Interface {
                file: banners.voice_file(v).map(str::to_string),
                what: v.label(),
            })
            .collect();
        // client-hudgaps: the low-health heartbeats (silent when the ROM table names no file)
        let beat = self.hud.layout.low_health.beat_file.clone();
        for _ in 0..std::mem::take(&mut self.state.low_health.beats) {
            if !self.hud.layout.from_rom || beat.is_some() {
                cues.push(crate::sound::SoundCue::Interface {
                    file: beat.clone(),
                    what: "heartbeat",
                });
            }
        }
        // the board window's open sound (the section of the character sounds; silent when the table names no file)
        let file = self.hud.layout.board_sound_file.clone();
        for _ in 0..std::mem::take(&mut self.state.board_opens) {
            if !self.hud.layout.from_rom || file.is_some() {
                cues.push(crate::sound::SoundCue::Interface {
                    file: file.clone(),
                    what: "board_open",
                });
            }
        }
        cues
    }

    /// The composed image when it is due (an input changed, or [`RECOMPOSE_MS`] passed); `force` always composes.
    pub fn image(&mut self, force: bool) -> Option<RgbaImage> {
        let items = self.items_due(force)?;
        let (w, h) = self.hud.size();
        Some(compose(&items, &mut self.hud.assets, w, h))
    }
}

// ---- render-overlay: the HUD as a batched display list on the GPU (no CPU raster per frame)
impl HudDriver {
    /// Draws `items` over the HUD (the report form; empty: nothing), with the next display list.
    pub fn set_overlay(&mut self, items: Vec<DrawItem>) {
        if self.hud.overlay != items || !items.is_empty() {
            self.hud.overlay = items;
            self.dirty = true;
        }
    }

    /// CLI-1604: the software cursor item (`None`: none), drawn last with the next display list.
    pub fn set_cursor(&mut self, item: Option<DrawItem>) {
        if self.hud.cursor != item {
            self.hud.cursor = item;
            self.dirty = true;
        }
    }

    /// Shows the leave box as `view` (`None`: closed), drawn with the next display list.
    pub fn set_leave_box(&mut self, view: Option<crate::loading::LeaveView>) {
        if self.state.leave_box != view {
            self.state.leave_box = view;
            self.dirty = true;
        }
    }

    /// The display list of the current state when it is due (the rule of [`HudDriver::image`]: an input changed,
    /// or [`RECOMPOSE_MS`] passed; `force` always), for [`crate::overlay::OverlayRenderer::update_items`].
    pub fn items_due(&mut self, force: bool) -> Option<Vec<DrawItem>> {
        let now = self.state.now_ms;
        let due = force
            || self.dirty
            || self
                .last_compose
                .is_none_or(|t| now.saturating_sub(t) >= RECOMPOSE_MS);
        if !due {
            return None;
        }
        self.dirty = false;
        self.last_compose = Some(now);
        self.composed += 1;
        Some(self.hud.items(&self.state))
    }

    /// Hands the display list to `layer` when it is due; `true` when it did. The frame loops call this instead of
    /// composing an image ([`HudDriver::image`] stays for callers that need the pixels).
    pub fn draw_into(
        &mut self,
        ctx: &dc_render::GpuContext,
        layer: &mut crate::overlay::OverlayRenderer,
        force: bool,
    ) -> bool {
        let Some(items) = self.items_due(force) else {
            return false;
        };
        let size = self.hud.size();
        layer.update_items(ctx, &items, &mut self.hud.assets, size);
        true
    }
}

#[cfg(test)]
mod sound_tests {
    use super::*;
    use crate::sound::SoundCue;
    use banners::Voice;

    fn driver() -> HudDriver {
        HudDriver::new(Hud::fallback(640, 480, None))
    }

    fn whats(d: &mut HudDriver) -> Vec<&'static str> {
        d.take_sound_cues()
            .into_iter()
            .map(|c| match c {
                SoundCue::Interface { what, .. } => what,
                _ => "other",
            })
            .collect()
    }

    #[test]
    fn announcer_cues_reach_the_sound_path() {
        let mut d = driver();
        d.state
            .banner
            .voices
            .extend([Voice::RoundStart, Voice::WinBravo]);
        assert_eq!(whats(&mut d), ["round_start", "round_win_second_team"]);
        assert!(whats(&mut d).is_empty(), "taken once");
    }

    #[test]
    fn the_board_key_cues_the_open_sound_once_and_the_close_is_silent() {
        let mut d = driver();
        let fs = crate::world::FrameState::default();
        d.update(&fs, 0, 0.0, false, true);
        assert_eq!(whats(&mut d), ["board_open"]);
        d.update(&fs, 16, 0.0, false, true);
        d.update(&fs, 32, 0.0, false, false);
        assert!(whats(&mut d).is_empty(), "held and closed: no new cue");
        d.update(&fs, 48, 0.0, false, true);
        assert_eq!(whats(&mut d), ["board_open"]);
    }

    #[test]
    fn the_board_text_uses_the_face_the_menu_draws_the_same_record_with() {
        // CLI-658: the in-match board named the ROM record's font but drew it with another face than the result
        // screen's text of the same record
        let Ok(rom) = std::env::var("CA_ROM") else {
            println!("skipped: CA_ROM unset");
            return;
        };
        let rom = std::path::Path::new(&rom);
        let mut hud = Hud::from_rom(rom, 1024, 768, None).expect("ROM HUD");
        let mut cat = crate::menu::catalog::Catalog::from_rom(rom, 1024, 768).expect("catalog");
        let (font, size) = (hud.layout.board.font, hud.layout.board.font_size);
        let w = |a: Option<std::sync::Arc<crate::overlay::GlyphAtlas>>| {
            a.expect("atlas").measure("Player Rank / Name 0123456789")
        };
        let hud_w = w(hud.assets.fonts.atlas_sized(font, size));
        let menu_w = w(cat.assets.fonts.atlas_sized(font, size));
        assert!(
            (hud_w - menu_w).abs() < 0.01,
            "board width {hud_w} vs menu {menu_w}"
        );
    }

    #[test]
    fn the_board_open_sound_key_resolves_in_the_rom() {
        let Ok(rom) = std::env::var("CA_ROM") else {
            println!("skipped: CA_ROM unset");
            return;
        };
        let f = vitals::load_board_sound(std::path::Path::new(&rom));
        println!("board open sound: {f:?}");
        assert!(f.is_some(), "the pinned key resolves to a file");
    }

    /// The user's own net log (dc-client.net.jsonl, 2026-11-04): a shot's 0x74 (damage 38, dmg_kind 3, zone 2) and a
    /// partial fall's (damage 33, dmg_kind 0, zone 5, vertical from_dir) both light the aim cross's hit flash through
    /// the driver's frame path, in the default mode (no Reality).
    #[test]
    fn a_shot_and_a_partial_fall_both_flash_the_aim_cross_in_default_mode() {
        use crate::spread::AimMarkView;
        use crate::world::FrameState;
        let mark = AimMarkView {
            shape: crate::spread::mark::SHAPE_PLAIN,
            distance: 10.0,
            since_shot_s: None,
            colour: [0, 255, 0, 255],
            rest: 10.0,
        };
        let colour = |d: &mut HudDriver| -> [u8; 4] {
            let items = d.items_due(true).expect("items");
            items
                .iter()
                .find_map(|i| match i {
                    DrawItem::AimMark(v) => Some(v.colour),
                    _ => None,
                })
                .expect("the aim mark")
        };
        for (name, dir, kind, zone, damage) in [
            ("shot", [-0.2686, -0.0981, 0.2541], 3u8, 2u8, 38u32),
            ("fall", [0.0, 0.3315, 0.0], 0, 5, 33),
        ] {
            let mut d = driver();
            let frame = |ms: u32, events: Vec<(u32, state::HudEvent)>| FrameState {
                clock_ms: Some(ms),
                aim_mark: Some(mark),
                hud_events: events,
                ..FrameState::default()
            };
            d.update(
                &frame(1000, vec![(1000, state::HudEvent::Life(1))]),
                1000,
                0.0,
                false,
                false,
            );
            let rest = colour(&mut d);
            d.update(
                &frame(
                    1010,
                    vec![
                        (1010, state::HudEvent::Damage(dir)),
                        (1010, state::HudEvent::Hit { damage, zone }),
                        (1010, state::HudEvent::Wound { damage, kind }),
                    ],
                ),
                1010,
                0.0,
                false,
                false,
            );
            let hit = colour(&mut d);
            assert_ne!(hit, rest, "{name}: the flash lights the aim mark");
            d.update(&frame(1500, vec![]), 1500, 0.0, false, false);
            assert_eq!(colour(&mut d), rest, "{name}: and fades");
        }
    }

    /// CLI-1847 (user decision 2026-11-04): the logged shot (38, kind 3, zone 2) and the logged partial fall (33,
    /// kind 0, zone 5, vertical `from_dir`) give the same set of effects: the aim-cross flash always; with Reality the
    /// damage arcs and the camera jar, none of them without it.
    #[test]
    fn the_logged_shot_and_fall_have_the_same_effects_with_reality_on_and_off() {
        let mut flags = vec![false; crate::camfx::NAME_COUNT];
        flags[2] = true;
        let effects = |reality: bool, dir: [f32; 3], dtype: u8, zone: u8, damage: u32| {
            let mut d = driver();
            d.set_reality(reality);
            d.state.damage_at = 1010;
            let ev = |e| (1010, e);
            let fs = crate::world::FrameState {
                clock_ms: Some(1010),
                hud_events: vec![
                    ev(state::HudEvent::Life(1)),
                    ev(state::HudEvent::Damage(dir)),
                    ev(state::HudEvent::Hit { damage, zone }),
                ],
                ..crate::world::FrameState::default()
            };
            d.update(&fs, 1010, 0.0, false, false);
            let mut cam = crate::camfx::CameraFx::new(
                reality,
                crate::camfx::DamageJar::from_flags(flags.clone()),
            );
            let jarred = cam.jar_hit(1010.0, dir, dtype, damage, 0.0).is_some() && cam.jars > 0;
            (
                d.state.hit_flash() > 0.0,
                // the shot's arcs light a sector; the fall's border (CLI-1847) lights none
                d.state.damage.iter().any(|v| *v > 0.0) || d.state.fall_border > 0.0,
                jarred,
            )
        };
        for reality in [false, true] {
            let shot = effects(reality, [-0.2686, -0.0981, 0.2541], 2, 2, 38);
            let fall = effects(reality, [0.0, 0.3315, 0.0], 5, 5, 33);
            assert_eq!(shot, (true, reality, reality), "shot, reality {reality}");
            assert_eq!(fall, shot, "fall, reality {reality}");
        }
    }

    /// CLI-1847 (user decision 2026-11-04: "the damage circle when you take fall damage is too small, it's in the
    /// middle of the screen rather than the border"), the user's own net log: the logged falls 0x74 (damage 3 and 33,
    /// kind 0, zone 5, `from_dir` [0, 0.03 / 0.33, 0]) start the red screen-edge band at full strength that is gone
    /// after one second and light no sector; the logged shot (38, kind 3, zone 2) lights its sector and no band.
    #[test]
    fn a_fall_draws_the_edge_border_and_no_sector_while_a_shot_draws_its_sector_and_no_border() {
        use crate::world::FrameState;
        // (border fills, sector arcs) in the frame's display list
        let count = |d: &mut HudDriver| -> (usize, usize) {
            let items = d.items_due(true).expect("items");
            let border = items
                .iter()
                .filter(|i| matches!(i, DrawItem::Image { tex, tint, .. } if tex == fallborder::TEXTURE && tint[3] > 0))
                .count();
            let arcs = items
                .iter()
                .filter(|i| matches!(i, DrawItem::Image { turn: Some(_), .. }))
                .count();
            (border, arcs)
        };
        for (name, dir, kind, zone, damage, fall) in [
            ("fall 3 HP", [0.0, 0.03, 0.0], 0u8, 5u8, 3u32, true),
            ("fall 33 HP", [0.0, 0.33, 0.0], 0, 5, 33, true),
            ("shot", [-0.2686, -0.0981, 0.2541], 3, 2, 38, false),
        ] {
            let mut d = driver();
            d.set_reality(true);
            let frame = |ms: u32, events: Vec<(u32, state::HudEvent)>| FrameState {
                clock_ms: Some(ms),
                hud_events: events,
                ..FrameState::default()
            };
            d.update(
                &frame(1000, vec![(1000, state::HudEvent::Life(1))]),
                1000,
                0.0,
                false,
                false,
            );
            assert_eq!(count(&mut d), (0, 0), "{name}: nothing before the hit");
            d.update(
                &frame(
                    1010,
                    vec![
                        (1010, state::HudEvent::Damage(dir)),
                        (1010, state::HudEvent::Hit { damage, zone }),
                        (1010, state::HudEvent::Wound { damage, kind }),
                    ],
                ),
                1010,
                0.0,
                false,
                false,
            );
            let (border, arcs) = count(&mut d);
            if fall {
                assert!(border > 0, "{name}: the band is drawn");
                assert_eq!(arcs, 0, "{name}: no sector lights");
                assert!(
                    d.state.fall_border >= 0.98,
                    "{name}: full strength at the hit: {}",
                    d.state.fall_border
                );
                // it decays and is gone by one second after the hit
                d.update(&frame(1510, vec![]), 1510, 0.0, false, false);
                let half = d.state.fall_border;
                assert!((0.45..0.55).contains(&half), "{name}: half way: {half}");
                assert!(count(&mut d).0 > 0);
                d.update(&frame(2020, vec![]), 2020, 0.0, false, false);
                assert_eq!(d.state.fall_border, 0.0, "{name}: gone");
                assert_eq!(count(&mut d), (0, 0), "{name}: and not drawn");
            } else {
                assert_eq!(border, 0, "{name}: no border");
                assert_eq!(arcs, 1, "{name}: its one sector");
                assert_eq!(d.state.fall_border, 0.0);
            }
        }
    }

    /// CLI-1847: one headless frame (the CPU compositor the GPU overlay mirrors) with the vignette: pixels at all
    /// four edges are red-tinted below the peak alpha, fainter further in, the centre and the inner part are
    /// untouched.
    #[test]
    fn the_fall_border_tints_the_edges_and_leaves_the_centre() {
        use crate::world::FrameState;
        let (w, h) = (640u32, 480u32);
        let mut d = driver();
        d.set_reality(true);
        let frame = |ms: u32, events: Vec<(u32, state::HudEvent)>| FrameState {
            clock_ms: Some(ms),
            hud_events: events,
            ..FrameState::default()
        };
        d.update(
            &frame(1000, vec![(1000, state::HudEvent::Life(1))]),
            1000,
            0.0,
            false,
            false,
        );
        let plain = d.image(true).unwrap();
        d.update(
            &frame(
                1010,
                vec![
                    (1010, state::HudEvent::Damage([0.0, 0.33, 0.0])),
                    (
                        1010,
                        state::HudEvent::Hit {
                            damage: 33,
                            zone: 5,
                        },
                    ),
                ],
            ),
            1010,
            0.0,
            false,
            false,
        );
        let hit = d.image(true).unwrap();
        for (name, x, y) in [
            ("left", 1, h / 2),
            ("right", w - 2, h / 2),
            ("top", w / 2, 1),
            ("bottom", w / 2, h - 2),
        ] {
            let p = hit.pixel(x, y);
            assert!(
                p[3] >= 80
                    && p[3] as f32 <= fallborder::PEAK_ALPHA
                    && p[0] > p[1] + 100
                    && p[0] > p[2] + 100,
                "{name}: red-tinted at the edge: {p:?}"
            );
            assert_ne!(p, plain.pixel(x, y), "{name}");
        }
        // fading inward: deeper pixels are fainter, and the inner part of the frame is untouched
        let (e, m) = (hit.pixel(1, h / 2), hit.pixel(w / 16, h / 2));
        assert!(m[3] > 0 && m[3] < e[3], "{m:?} vs {e:?}");
        for (x, y) in [
            (w / 2, h / 2),
            (w * 3 / 8, h / 2),
            (w / 2, h / 2 + 100),
            (w / 4, h / 2),
        ] {
            assert_eq!(hit.pixel(x, y), plain.pixel(x, y), "({x}, {y}) untouched");
        }
        // gone a second later
        d.update(&frame(2100, vec![]), 2100, 0.0, false, false);
        let later = d.image(true).unwrap();
        assert_eq!(later.pixel(1, h / 2), plain.pixel(1, h / 2));
    }

    #[test]
    fn reality_setter_gates_the_arcs() {
        let mut d = driver();
        d.state.life_state = 1;
        d.state.on_hit([-1.0, 0.0, 0.0], 0.0);
        assert!(d.state.damage.iter().all(|v| *v == 0.0), "off by default");
        d.set_reality(true);
        d.state.on_hit([-1.0, 0.0, 0.0], 0.0);
        assert!(d.state.damage.iter().any(|v| *v > 0.0));
    }

    /// The caret of the open chat input sits after the typed line: the line is among the texts measured for `build`
    /// (review fix: it was not, and the caret stayed on the first character).
    #[test]
    fn chat_caret_after_typed_line() {
        let layout = HudLayout::fallback();
        let mut state = HudState::default();
        state.chat_input.open(chat::ChatScope::All, 0);
        state.chat_input.type_text("hello");
        let keys = measured_texts(&layout, &state);
        let measure = |f: i32, t: &str| -> f32 {
            if keys.iter().any(|(kf, kt)| *kf == f && kt == t) {
                t.chars().count() as f32 * 7.0
            } else {
                0.0
            }
        };
        let list = items::build(&layout, &state, &measure, &|_| None);
        let caret = list
            .iter()
            .find_map(|i| match i {
                items::DrawItem::Text { text, pos, .. } if text == "|" => Some(*pos),
                _ => None,
            })
            .expect("caret drawn at the opening instant");
        let [x, y] = layout.chat_look.input_pos;
        assert_eq!(caret, [x as f32 + 35.0, y as f32]);
    }
}
