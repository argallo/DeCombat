//! Search and Destroy on the client (mode classes 2 / 3): the bomb messages 0x51 / 0x50 / 0x4f (bomb timer, notices,
//! planted sound, the timer beeps), the carrier mark 0x56, the hold-to-use gauge of 0xad, and the use key held at a
//! plant or defuse point (`UP_USE_HOLD` 0xd8 phase 1 / 4).
//!
//! The 2009 bomb timer element draws the icon and six digit images (minutes, seconds, hundredths) at fixed places
//! (cshell+0x6ffd0) and plays one beep per whole second whose sound key depends on the time left (cshell+0x6fda0).
// SPEC: GAME_MESSAGES_2009_S2C.md §2.2 rows 0x4f / 0x50 / 0x51 / 0x56, §4.4 row 0xad; GAME_MODES_2009.md §4.3, §4.5,
// §5.2; GAME_MESSAGES_2009_C2S.md §7.1
// EVIDENCE: cshell+0x6ffd0 (bomb timer items and places), cshell+0x6fc60 (digits: minutes / 10, minutes % 10,
// seconds / 10, seconds % 10, hundredths / 10, hundredths % 10), cshell+0x6fda0 (one beep per whole second: key 1
// above 30 s, 2 above 20, 3 above 10, 4 above 5, 5 above 0), cshell+0x4afee (0x51: the planted sound key, the
// timer, the planted notice), cshell+0x4aeab / 0x4adfa (0x50 / 0x4f: timer hidden, their notices), cshell+0xafe30
// (the gauge: nine frames, the plant / defuse label), cshell+0x9b0c0 (the held objective item image)

use super::banners::Voice;
use super::items::DrawItem;
use super::layout::Img;
use super::state::HudState;
use dc_ui_data::{HudKey as K, HudLiteral, HudView};

/// One bomb message for the HUD.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BombEvent {
    /// 0x51 `[bomb_secs]`.
    Planted(u32),
    /// 0x50.
    Defused,
    /// 0x4f.
    Exploded,
    /// 0x56 `[item:objref16]` (DeCombat: the carrier's character, `None` while the bomb lies dropped).
    Carrier(Option<u16>),
    /// 0xad `[obj][kind][a][hold_secs][c]`: the hold on `obj` runs `secs`; `kind` 2 = a defuse.
    Track { obj: u16, kind: u8, secs: f32 },
    /// The own client let go of the use key (or died) during a hold (no message: the client's own event).
    Released,
}

/// Which notice shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BombNotice {
    Planted,
    Defused,
    Exploded,
}

/// How long a bomb notice stays.
// DC-DEFAULT: CLI-1053 (proposed) 3000 ms (the 0xbcb text of a timer-less 0x51 shows 3000 ms, S2C §2.2)
pub const NOTICE_MS: u32 = 3000;

/// The bomb timer element's fixed places (virtual 1024 x 768): the icon, the six digits, the two separators.
// EVIDENCE: cshell+0x6ffd0 (icon 0x19 x 0x1f at (0x1a5, 0x3d); digits 0x12 x 0x1c at y 0x3f: minutes 0x1bf / 0x1cf,
// separators 0x1df / 0x20f, seconds 0x1ef / 0x1ff, hundredths 0x21f / 0x22f)
pub const ICON_AT: [i32; 2] = [0x1a5, 0x3d];
pub const ICON_SIZE: [i32; 2] = [0x19, 0x1f];
pub const DIGIT_SIZE: [i32; 2] = [0x12, 0x1c];
pub const DIGIT_Y: i32 = 0x3f;
pub const DIGIT_X: [i32; 6] = [0x1bf, 0x1cf, 0x1ef, 0x1ff, 0x21f, 0x22f];
pub const SEPARATOR_X: [i32; 2] = [0x1df, 0x20f];
/// The held-bomb image's size when the texture's own size (the 2009 element's) is not known.
// DC-DEFAULT: CLI-1052 (proposed) 64 x 64
pub const HELD_SIZE: [i32; 2] = [64, 64];
/// Number of gauge frames (the gauge frame key is a form with one `%d`).
// EVIDENCE: cshell+0xafe30 (frames 0..8 loaded)
pub const GAUGE_FRAMES: u32 = 9;

/// The S&D HUD state.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BombHud {
    /// The bomb timer: when it started and its seconds.
    pub planted: Option<(u32, u32)>,
    pub notice: Option<(BombNotice, u32)>,
    /// The object 0x56 marked (the carrier's character).
    pub carrier: Option<u16>,
    /// The running hold: defuse or plant, start, seconds.
    pub track: Option<(bool, u32, f32)>,
    /// The whole second of the last beep.
    pub beep_sec: Option<u32>,
}

/// The textures and places the S&D HUD reads.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BombLayout {
    pub digits: String,
    pub icon: String,
    pub separator: String,
    /// Planted, defused, exploded notices.
    pub notices: [Option<Img>; 3],
    pub gauge_frames: String,
    pub gauge: Option<Img>,
    /// Plant and defuse labels at the label place.
    pub labels: [Option<Img>; 2],
    pub held: Option<Img>,
}

fn img(v: &HudView<'_>, tex: K, pos: K, size: K) -> Option<Img> {
    Some(Img {
        tex: v.text(tex)?.to_string(),
        pos: v.pair(pos)?,
        size: v.pair(size)?,
    })
}

impl BombLayout {
    pub fn load(v: &HudView<'_>) -> Self {
        let lit = |l: HudLiteral| v.keys.literal(l).to_string();
        BombLayout {
            digits: lit(HudLiteral::BombDigits),
            icon: lit(HudLiteral::BombIcon),
            separator: lit(HudLiteral::BombSeparator),
            notices: [
                img(
                    v,
                    K::BombPlantedMsg,
                    K::BombPlantedMsgPos,
                    K::BombPlantedMsgSize,
                ),
                img(
                    v,
                    K::BombDefusedMsg,
                    K::BombDefusedMsgPos,
                    K::BombDefusedMsgSize,
                ),
                img(
                    v,
                    K::BombExplodedMsg,
                    K::BombExplodedMsgPos,
                    K::BombExplodedMsgSize,
                ),
            ],
            gauge_frames: v.text(K::GaugeFrames).unwrap_or_default().to_string(),
            gauge: img(v, K::GaugeFrames, K::GaugePos, K::GaugeSize),
            labels: [
                img(v, K::GaugePlantLabel, K::GaugeLabelPos, K::GaugeLabelSize),
                img(v, K::GaugeDefuseLabel, K::GaugeLabelPos, K::GaugeLabelSize),
            ],
            held: v
                .text(K::HeldBombTex)
                .zip(v.pair(K::HeldBombPos))
                .map(|(t, p)| Img {
                    tex: t.to_string(),
                    pos: p,
                    size: HELD_SIZE,
                }),
        }
    }
}

/// The beep voice for `left` seconds of the bomb timer.
// EVIDENCE: cshell+0x6fda0 (thresholds 30, 20, 10, 5, 0: the float defaults of its key reads)
pub fn beep_for(left: f32) -> Option<Voice> {
    Some(if left > 30.0 {
        Voice::BombBeep1
    } else if left > 20.0 {
        Voice::BombBeep2
    } else if left > 10.0 {
        Voice::BombBeep3
    } else if left > 5.0 {
        Voice::BombBeep4
    } else if left > 0.0 {
        Voice::BombBeep5
    } else {
        return None;
    })
}

/// The digits of the bomb timer for `left` seconds: minutes, seconds, hundredths, two digits each.
// EVIDENCE: cshell+0x6fc60
pub fn timer_digits(left: f32) -> [u32; 6] {
    let left = left.max(0.0);
    let s = left as u32;
    let c = ((left - s as f32) * 100.0) as u32 % 100;
    let m = s / 60;
    [(m / 10) % 10, m % 10, (s % 60) / 10, s % 10, c / 10, c % 10]
}

impl HudState {
    /// The bomb messages: 0x51 starts the timer, the planted notice and sound and ends any hold; 0x50 / 0x4f hide
    /// the timer and show their notices; 0x56 keeps the marked object; 0xad starts the gauge.
    // DC-DEFAULT: CLI-1051 / CLI-1053 (proposed)
    pub fn on_bomb(&mut self, e: &BombEvent, now: u32) {
        // after the round result a late bomb message changes nothing: no timer, notice, sound or gauge over the
        // round-end screen (the server cancels a planted bomb when the round ends by another cause)
        // DC-DEFAULT: CLI-1243 (proposed, playtest report "it should stop counting down on the round end screen")
        if self.between_rounds
            && matches!(
                e,
                BombEvent::Planted(_)
                    | BombEvent::Defused
                    | BombEvent::Exploded
                    | BombEvent::Track { .. }
            )
        {
            return;
        }
        match *e {
            BombEvent::Planted(secs) => {
                // the round clock is set to 0 and hidden (the bomb timer takes its place); the next 0x44 shows it
                // SPEC: GAME_MESSAGES_2009_S2C.md §2.2 row 0x51 (round timer set to 0 and hidden)
                // EVIDENCE: cshell+0x4afee..0x4b23f (0x51 handler)
                // DC-DEFAULT: CLI-1242 (proposed, playtest report "the round counter should stop")
                self.set_clock(0, now, false, false);
                self.bomb.planted = (secs > 0).then_some((now, secs));
                self.bomb.notice = Some((BombNotice::Planted, now));
                self.bomb.track = None;
                self.bomb.beep_sec = None;
                self.banner.voices.push(Voice::BombSet);
            }
            BombEvent::Defused => {
                self.bomb.planted = None;
                self.bomb.track = None;
                self.bomb.notice = Some((BombNotice::Defused, now));
            }
            BombEvent::Exploded => {
                self.bomb.planted = None;
                self.bomb.track = None;
                self.bomb.notice = Some((BombNotice::Exploded, now));
            }
            BombEvent::Carrier(obj) => self.bomb.carrier = obj,
            BombEvent::Track { kind, secs, .. } => {
                self.bomb.track = Some((kind == 2, now, secs));
            }
            BombEvent::Released => self.bomb_hold_released(),
        }
    }

    /// The bomb timer's beep once per whole second while it runs; the gauge ends with its time.
    pub fn tick_bomb(&mut self, now: u32) {
        if let Some((at, secs)) = self.bomb.planted {
            let left = secs as f32 - now.saturating_sub(at) as f32 / 1000.0;
            let whole = left.max(0.0).ceil() as u32;
            if left > 0.0 && self.bomb.beep_sec != Some(whole) {
                self.bomb.beep_sec = Some(whole);
                if let Some(v) = beep_for(left) {
                    self.banner.voices.push(v);
                }
            }
        }
        if let Some((_, at, secs)) = self.bomb.track {
            if now.saturating_sub(at) as f32 >= secs * 1000.0 {
                self.bomb.track = None;
            }
        }
    }

    /// The own hold ended on this client (key let go, death): the gauge goes.
    pub fn bomb_hold_released(&mut self) {
        self.bomb.track = None;
    }

    /// The round ended (0x46 result, 0x48 / 0x41 close, 0x3f result screen): the bomb timer, its beeps and the
    /// gauge stop at once; the carrier mark stays until the next round.
    // DC-DEFAULT: CLI-1243 (proposed, playtest report)
    pub fn bomb_round_end(&mut self) {
        self.bomb.planted = None;
        self.bomb.track = None;
        self.bomb.beep_sec = None;
    }

    /// A new round: no timer, no gauge, no carrier mark.
    pub fn bomb_round_reset(&mut self) {
        self.bomb.planted = None;
        self.bomb.track = None;
        self.bomb.carrier = None;
        self.bomb.beep_sec = None;
    }
}

/// One bomb image: the control's size in texels from the texture's top-left corner, drawn 1:1 into its place (the
/// ROM pictures sit in the top-left of larger power-of-two textures: the gauge frames in 256 x 256 at 145 x 137, the
/// labels in 256 x 128 at 155 x 114, the timer icon and digits in 32 x 32 at 25 x 31 and 18 x 28).
// EVIDENCE: cshell+0x343860 (the control's size kept as the source rect 0, 0, w, h), cshell+0x342d10 (one-quad
// branch, flag 1: texture coordinates = that rect / the texture's own size), cshell+0xafe30 / cshell+0x6ffd0 (the
// gauge, labels, timer icon and digits set with the control sizes)
// DC-DEFAULT: CLI-1241 (proposed, playtest report "planting progress mis-scaled"; replaces CLI-1190's whole-texture rule)
fn image(i: &Img, tex: Option<String>) -> DrawItem {
    DrawItem::Image {
        tex: tex.unwrap_or_else(|| i.tex.clone()),
        src: [i.size[0] as f32, i.size[1] as f32],
        dst: [
            i.pos[0] as f32,
            i.pos[1] as f32,
            i.size[0] as f32,
            i.size[1] as f32,
        ],
        tint: [255, 255, 255, 255],
        turn: None,
    }
}

fn at(tex: String, pos: [i32; 2], size: [i32; 2]) -> DrawItem {
    image(&Img { tex, pos, size }, None)
}

/// Draws the S&D parts of state `s` (nothing outside classes 2 / 3): the bomb timer, the notice and the gauge.
pub fn draw(
    out: &mut Vec<DrawItem>,
    l: &BombLayout,
    s: &HudState,
) {
    if !matches!(s.mode_class, Some(2 | 3)) {
        return;
    }
    let now = s.now_ms;
    if let Some((t0, secs)) = s.bomb.planted {
        let left = secs as f32 - now.saturating_sub(t0) as f32 / 1000.0;
        out.push(at(l.icon.clone(), ICON_AT, ICON_SIZE));
        for (x, d) in DIGIT_X.iter().zip(timer_digits(left)) {
            out.push(at(
                l.digits.replace("%d", &d.to_string()),
                [*x, DIGIT_Y],
                DIGIT_SIZE,
            ));
        }
        for x in SEPARATOR_X {
            out.push(at(l.separator.clone(), [x, DIGIT_Y], DIGIT_SIZE));
        }
    }
    if let Some((n, t0)) = s.bomb.notice {
        if now.saturating_sub(t0) < NOTICE_MS {
            if let Some(Some(i)) = l.notices.get(n as usize) {
                out.push(image(i, None));
            }
        }
    }
    if let (Some((defuse, t0, secs)), Some(g)) = (s.bomb.track, &l.gauge) {
        let f = (now.saturating_sub(t0) as f32 / (secs * 1000.0).max(1.0)).clamp(0.0, 1.0);
        // DC-DEFAULT: CLI-1052 (proposed) the frame by the share of the hold done
        let frame = ((f * GAUGE_FRAMES as f32) as u32).min(GAUGE_FRAMES - 1);
        out.push(image(
            g,
            Some(l.gauge_frames.replace("%d", &frame.to_string())),
        ));
        if let Some(Some(lb)) = l.labels.get(usize::from(defuse)) {
            out.push(image(lb, None));
        }
    }
    // The held-bomb image (`l.held`) is never drawn: 2009 showed it while the own player held the objective item,
    // and since DSV-350 every attacker holds the bomb all round, so the image would only repeat the side.
    // EVIDENCE: cshell+0x9b0c0 (shown while the objective item passes the can-change-to-weapon test cshell+0x22df10:
    // alive, not spectating, the weapon owned)
    // DC-DEFAULT: CLI-1240 (proposed, playtest report "the bomb icon in the top left shouldn't be there")
}

/// One plant or defuse point of the world (a type-0x37 zone of kind 5).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UsePoint {
    pub obj: u16,
    /// 1 plant, 2 defuse.
    pub sub: u8,
    pub pos: [f32; 3],
}

/// The kind-5 point of a zone blob (type byte first): `z_sub` 1 or 2, else `None`.
// SPEC: GAME_MODES_2009.md §4.3 (record order: a, b, flag, vec72, icon, team, class, c, value, kind, d, sub)
pub fn read_use_point(bytes: &[u8], bits: usize) -> Option<u8> {
    let mut r = dc_wire::udp::BitReader::with_bit_len(bytes, bits);
    if r.remaining_bits() < 8 + 64 + 1 + 72 + 32 + 96 + 8 || r.read_u8() != 0x37 {
        return None;
    }
    r.skip_bits(64 + 1 + 72 + 32 + 32);
    let kind = r.read_u32();
    r.skip_bits(32);
    let sub = r.read_u8();
    (kind == 5 && matches!(sub, 1 | 2)).then_some(sub)
}

/// The plant and defuse points among the client's objects.
pub fn use_points(
    objects: &std::collections::BTreeMap<u16, dc_botclient::objects::ClientObject>,
) -> Vec<UsePoint> {
    objects
        .iter()
        .filter(|(_, o)| o.blob_type == Some(0x37))
        .filter_map(|(id, o)| {
            let b = o.blob.as_ref()?;
            let sub = read_use_point(b.as_bytes(), b.bit_len())?;
            Some(UsePoint {
                obj: *id,
                sub,
                pos: o.pos.unwrap_or(o.create_pos),
            })
        })
        .collect()
}

/// The inventory slot the server grants the bomb item in (DSV-240): the slot whose weapon the 2009 client turns on
/// at a plant / defuse point in classes 2 / 3.
// EVIDENCE: cshell+0x5d230 -> cshell+0x22d2b0 (classes 2 / 3: inventory entry 9)
pub const BOMB_SLOT: u8 = 9;
/// The defending team (it defuses).
// SPEC: GAME_MODES_2009.md §5.2 (team 2 defends)
pub const DEFUSE_TEAM: u8 = 2;

/// Whether the drawn weapon is the bomb item: mode class 2 / 3 and the bomb slot in the hand. Then the fire key holds
/// the plant (the item fires no shot).
// DC-DEFAULT: CLI-1100 (proposed) the fire key with the bomb out is the plant hold (as the use key)
pub fn bomb_in_hand(mode_class: Option<u8>, held_slot: Option<u8>) -> bool {
    matches!(mode_class, Some(2 | 3)) && held_slot == Some(BOMB_SLOT)
}

/// While the own plant / defuse hold runs no weapon is chosen: the slot keys, the class keys and the wheel do nothing.
// EVIDENCE: cshell+0x48a70 (the weapon commands 0x45..0x53 return at once while the use progress at
// `local_player`+0xc runs, cshell+0x1f0bb0)
// DC-DEFAULT: CLI-1191 (proposed, playtest report) the running hold is the client's own sent hold
pub fn lock_weapon_change(c: &mut crate::arms::WeaponCommands, holding: bool) {
    if holding {
        c.select = None;
        c.class_key = None;
        c.wheel = 0;
    }
}

/// `UP_USE_HOLD` and its body `[phase:u8][target:objref16]`.
// SPEC: GAME_MESSAGES_2009_C2S.md §7.1
pub const UP_USE_HOLD: u8 = 0xd8;

pub fn use_hold_body(phase: u8, target: u16) -> dc_wire::udp::BitWriter {
    let mut b = dc_wire::udp::BitWriter::new();
    b.write_u8(phase).write_objref16(Some(target));
    b
}

/// The use key held at a point: phase 1 on the nearest point of the own side's kind (attackers plant, defenders
/// defuse) when the key goes down alive in classes 2 / 3, phase 4 when it is let go or the player died. `held` keeps
/// the target of the running hold. The server decides whether the place counts.
// SPEC: GAME_MESSAGES_2009_C2S.md §7.1 (phase 1 with a use target, phase 4 when the command goes off)
// DC-DEFAULT: CLI-1054 (proposed) the nearest point of the side's kind stands for the 2009 zone touch test
pub fn hold_step(
    held: &mut Option<u16>,
    pressed: bool,
    alive: bool,
    mode_class: Option<u8>,
    team: Option<u8>,
    at: [f32; 3],
    points: &[UsePoint],
) -> Option<(u8, u16)> {
    if let Some(t) = *held {
        if !pressed || !alive {
            *held = None;
            return Some((4, t));
        }
        return None;
    }
    if !pressed || !alive || !matches!(mode_class, Some(2 | 3)) {
        return None;
    }
    let sub = match team {
        Some(1) => 1,
        Some(2) => 2,
        _ => return None,
    };
    let d2 = |p: &UsePoint| (0..3).map(|i| (p.pos[i] - at[i]).powi(2)).sum::<f32>();
    let t = points
        .iter()
        .filter(|p| p.sub == sub)
        .min_by(|a, b| d2(a).total_cmp(&d2(b)))?
        .obj;
    *held = Some(t);
    Some((1, t))
}

/// The half extents a DeCombat server writes into a type-0x37 zone blob (type byte first): the record's vec72 after
/// `z_a`, `z_b` and `z_flag` (the zone's extent, DSV-37); `None` when it is not a zone blob.
// SPEC: GAME_MODES_2009.md §4.3 (record order: a, b, flag, vec72, ...); UDP_TRANSPORT_2009_addendum_disasm.md T15
// (vec72 decode)
pub fn read_zone_half(bytes: &[u8], bits: usize) -> Option<[f32; 3]> {
    let mut r = dc_wire::udp::BitReader::with_bit_len(bytes, bits);
    if r.remaining_bits() < 8 + 64 + 1 + 72 || r.read_u8() != 0x37 {
        return None;
    }
    r.skip_bits(64 + 1);
    let v = dc_botclient::objects::decode_vec72(dc_wire::udp::Vec72::read(&mut r));
    v.iter().all(|x| x.is_finite()).then(|| v.map(f32::abs))
}

/// The smallest half extent of a plant / defuse point's box in the own hold test.
// DC-DEFAULT: CLI-1250 (proposed) the server's minimum site half extent (DSV-200, 96 units; the defuse point is
// written with the server's 96-unit defuse reach, DSV-222)
pub const HOLD_POINT_MIN_HALF: f32 = 96.0;

/// Whether the server will take a hold on point `target` from `at`: inside the point's box (its zone extent, at
/// least [`HOLD_POINT_MIN_HALF`] on each axis, round the point's position). Only then does the own body stop at the
/// hold start (CLI-1250); a refused hold gets no answer from the server (DSV-202), so a stop there would last until
/// the key is let go.
// DC-DEFAULT: CLI-1250 (proposed)
pub fn hold_point_takes(
    objects: &std::collections::BTreeMap<u16, dc_botclient::objects::ClientObject>,
    target: u16,
    at: [f32; 3],
) -> bool {
    let Some(o) = objects.get(&target) else {
        return false;
    };
    let half = o
        .blob
        .as_ref()
        .and_then(|b| read_zone_half(b.as_bytes(), b.bit_len()))
        .unwrap_or([0.0; 3]);
    let pos = o.pos.unwrap_or(o.create_pos);
    (0..3).all(|i| (at[i] - pos[i]).abs() <= half[i].max(HOLD_POINT_MIN_HALF))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// CLI-1191: no weapon change while the own hold runs; free again after it.
    #[test]
    fn a_running_hold_blocks_the_weapon_change() {
        let base = crate::arms::WeaponCommands {
            select: Some(1),
            class_key: Some(0),
            wheel: 2,
            fire: true,
            ..Default::default()
        };
        let mut c = base;
        lock_weapon_change(&mut c, true);
        assert_eq!((c.select, c.class_key, c.wheel, c.fire), (None, None, 0, true));
        let mut c = base;
        lock_weapon_change(&mut c, false);
        assert_eq!(c, base);
    }

    /// CLI-1100: the bomb slot in the hand counts only in classes 2 / 3; with it out the fire key starts the plant
    /// hold at the nearest plant point and letting go ends it.
    #[test]
    fn the_bomb_in_the_hand_turns_the_fire_key_into_the_plant_hold() {
        assert!(bomb_in_hand(Some(2), Some(BOMB_SLOT)));
        assert!(bomb_in_hand(Some(3), Some(BOMB_SLOT)));
        assert!(!bomb_in_hand(Some(1), Some(BOMB_SLOT)));
        assert!(!bomb_in_hand(Some(2), Some(0)));
        let pts = [UsePoint {
            obj: 11,
            sub: 1,
            pos: [0.0; 3],
        }];
        let mut held = None;
        let fire = true;
        let pressed = false || (bomb_in_hand(Some(2), Some(BOMB_SLOT)) && fire);
        assert_eq!(
            hold_step(
                &mut held,
                pressed,
                true,
                Some(2),
                Some(1),
                [10.0, 0.0, 0.0],
                &pts
            ),
            Some((1, 11))
        );
        assert_eq!(
            hold_step(
                &mut held,
                false,
                true,
                Some(2),
                Some(1),
                [10.0, 0.0, 0.0],
                &pts
            ),
            Some((4, 11))
        );
    }

    fn layout() -> BombLayout {
        let im = |t: &str, x: i32| {
            Some(Img {
                tex: t.into(),
                pos: [x, 0],
                size: [10, 10],
            })
        };
        BombLayout {
            digits: "n_%d.dtx".into(),
            icon: "icon.dtx".into(),
            separator: "sep.dtx".into(),
            notices: [im("set.dtx", 1), im("defused.dtx", 2), im("boom.dtx", 3)],
            gauge_frames: "g_%d.dtx".into(),
            gauge: im("g_%d.dtx", 4),
            labels: [im("plant.dtx", 5), im("defuse.dtx", 6)],
            held: im("c4.dtx", 7),
        }
    }

    fn texs(out: &[DrawItem]) -> Vec<(String, f32)> {
        out.iter()
            .filter_map(|d| match d {
                DrawItem::Image { tex, dst, .. } => Some((tex.clone(), dst[0])),
                _ => None,
            })
            .collect()
    }

    /// CLI-1050..1053: 0x51 [40] draws the icon and 00:39:50 half a second later at the 2009 places, the planted
    /// notice for 3 s, the planted sound and one beep per whole second; 0x50 hides the timer and shows the defused
    /// notice; a TDM room draws none of it.
    #[test]
    fn bomb_timer_notice_and_beeps() {
        let mut s = HudState {
            mode_class: Some(2),
            ..HudState::default()
        };
        s.on_bomb(&BombEvent::Planted(40), 1000);
        assert_eq!(s.banner.voices, vec![Voice::BombSet]);
        s.tick(1500, 5000);
        s.tick_bomb(1500);
        assert_eq!(s.banner.voices.last(), Some(&Voice::BombBeep1));
        let n = s.banner.voices.len();
        s.tick_bomb(1700);
        assert_eq!(s.banner.voices.len(), n, "one beep per second");
        let mut out = Vec::new();
        draw(&mut out, &layout(), &s);
        let t = texs(&out);
        for want in [
            ("icon.dtx", 421.0),
            ("n_0.dtx", 447.0),
            ("n_0.dtx", 463.0),
            ("n_3.dtx", 495.0),
            ("n_9.dtx", 511.0),
            ("n_5.dtx", 543.0),
            ("n_0.dtx", 559.0),
            ("sep.dtx", 479.0),
            ("sep.dtx", 527.0),
            ("set.dtx", 1.0),
        ] {
            assert!(t.contains(&(want.0.to_string(), want.1)), "{want:?} {t:?}");
        }
        // the last seconds beep with the fifth key
        s.tick_bomb(1000 + 37_500);
        assert_eq!(s.banner.voices.last(), Some(&Voice::BombBeep5));
        s.on_bomb(&BombEvent::Defused, 2000);
        s.now_ms = 2100;
        let mut out = Vec::new();
        draw(&mut out, &layout(), &s);
        let t = texs(&out);
        assert!(!t.iter().any(|x| x.0 == "icon.dtx"));
        assert!(t.contains(&("defused.dtx".into(), 2.0)));
        s.mode_class = Some(1);
        let mut out = Vec::new();
        draw(&mut out, &layout(), &s);
        assert!(out.is_empty());
    }

    /// CLI-1052: 0xad shows the gauge frame by the share done with the plant / defuse label. CLI-1240: the held
    /// bomb is never drawn, also when 0x56 marks the own character (the mark is still kept).
    #[test]
    fn gauge_and_no_held_bomb() {
        let mut s = HudState {
            mode_class: Some(3),
            ..HudState::default()
        };
        s.on_bomb(
            &BombEvent::Track {
                obj: 5,
                kind: 1,
                secs: 4.0,
            },
            0,
        );
        s.on_bomb(&BombEvent::Carrier(Some(9)), 0);
        s.view_object = Some(9);
        s.now_ms = 2000;
        assert_eq!(s.bomb.carrier, Some(9));
        let mut out = Vec::new();
        draw(&mut out, &layout(), &s);
        let t = texs(&out);
        assert!(t.contains(&("g_4.dtx".into(), 4.0)), "{t:?}");
        assert!(t.contains(&("plant.dtx".into(), 5.0)));
        assert!(!t.iter().any(|x| x.0 == "c4.dtx"), "no held bomb: {t:?}");
        s.tick_bomb(4000);
        assert!(s.bomb.track.is_none(), "the gauge ends with its time");
    }

    /// CLI-1242: 0x51 hides the round clock (set to 0) while the bomb timer runs; the next 0x44 shows it again.
    #[test]
    fn the_planted_bomb_hides_the_round_clock() {
        let mut s = HudState {
            mode_class: Some(2),
            ..HudState::default()
        };
        s.on_round_begin(120, 0);
        s.now_ms = 1000;
        assert!(s.clock_left().is_some());
        s.on_bomb(&BombEvent::Planted(40), 1000);
        s.now_ms = 3000;
        assert_eq!(s.clock_left(), None, "no round clock beside the bomb timer");
        assert_eq!(s.clock.secs, 0);
        assert!(s.bomb.planted.is_some());
        s.on_round_begin(120, 60_000);
        s.now_ms = 60_000;
        assert_eq!(s.clock_left(), Some(120), "back at the next round");
        assert!(s.bomb.planted.is_none());
    }

    /// CLI-1243: the round result stops the bomb timer and its beeps at once; a 0x4f / 0x50 / 0x51 after it shows
    /// no notice and plays nothing; the next 0x44 takes bomb messages again.
    #[test]
    fn the_round_end_stops_the_bomb_timer() {
        let mut s = HudState {
            mode_class: Some(2),
            ..HudState::default()
        };
        s.on_round_begin(120, 0);
        s.on_bomb(&BombEvent::Planted(40), 1000);
        s.tick_bomb(1500);
        assert!(s.banner.voices.contains(&Voice::BombBeep1));
        s.on_round_result(5, 2, [0, 1], 10_000);
        assert!(s.bomb.planted.is_none(), "timer stopped at the round end");
        let n = s.banner.voices.len();
        s.tick_bomb(12_000);
        s.tick_bomb(13_000);
        assert_eq!(s.banner.voices.len(), n, "no beeps over the round-end screen");
        s.now_ms = 13_000;
        let mut out = Vec::new();
        draw(&mut out, &layout(), &s);
        assert!(!texs(&out).iter().any(|x| x.0 == "icon.dtx" || x.0.starts_with("n_")));
        let notice = s.bomb.notice;
        s.on_bomb(&BombEvent::Exploded, 13_000);
        s.on_bomb(&BombEvent::Defused, 13_000);
        s.on_bomb(&BombEvent::Planted(40), 13_000);
        assert_eq!(s.bomb.notice, notice, "no stale notice after the round end");
        assert!(s.bomb.planted.is_none());
        assert_eq!(s.banner.voices.len(), n);
        s.on_round_begin(120, 20_000);
        s.on_bomb(&BombEvent::Planted(40), 21_000);
        assert!(s.bomb.planted.is_some(), "the next round takes the bomb again");
    }

    #[test]
    fn timer_digits_split() {
        assert_eq!(timer_digits(39.5), [0, 0, 3, 9, 5, 0]);
        assert_eq!(timer_digits(75.25), [0, 1, 1, 5, 2, 5]);
        assert_eq!(timer_digits(-1.0), [0; 6]);
    }

    /// CLI-1054: the key down at a plant point sends phase 1 on the nearest plant point (an attacker), nothing in
    /// TDM or for a dead player; letting go sends phase 4 on the same target.
    #[test]
    fn use_key_holds_on_the_nearest_point_of_the_side() {
        let pts = [
            UsePoint {
                obj: 10,
                sub: 1,
                pos: [0.0, 0.0, 0.0],
            },
            UsePoint {
                obj: 11,
                sub: 2,
                pos: [5.0, 0.0, 0.0],
            },
            UsePoint {
                obj: 12,
                sub: 1,
                pos: [900.0, 0.0, 0.0],
            },
        ];
        let mut h = None;
        assert_eq!(
            hold_step(&mut h, true, true, Some(1), Some(1), [0.0; 3], &pts),
            None
        );
        assert_eq!(
            hold_step(&mut h, true, false, Some(2), Some(1), [0.0; 3], &pts),
            None
        );
        assert_eq!(
            hold_step(
                &mut h,
                true,
                true,
                Some(2),
                Some(1),
                [800.0, 0.0, 0.0],
                &pts
            ),
            Some((1, 12))
        );
        assert_eq!(
            hold_step(
                &mut h,
                true,
                true,
                Some(2),
                Some(1),
                [800.0, 0.0, 0.0],
                &pts
            ),
            None
        );
        assert_eq!(
            hold_step(
                &mut h,
                false,
                true,
                Some(2),
                Some(1),
                [800.0, 0.0, 0.0],
                &pts
            ),
            Some((4, 12))
        );
        // a defender gets the defuse point
        assert_eq!(
            hold_step(
                &mut h,
                true,
                true,
                Some(2),
                Some(2),
                [800.0, 0.0, 0.0],
                &pts
            ),
            Some((1, 11))
        );
        let body = use_hold_body(1, 11);
        assert_eq!(body.as_bytes(), &[1, 11, 0]);
    }

    #[test]
    fn a_zone_blob_of_kind_five_is_a_use_point() {
        let mut w = dc_wire::udp::BitWriter::new();
        w.write_u8(0x37).write_u32(0).write_u32(0).write_bit(false);
        for _ in 0..9 {
            w.write_u8(0);
        }
        w.write_u8(0xff).write_u8(0xfe).write_u8(2).write_u8(0);
        w.write_u32(1)
            .write_u32(5)
            .write_u32(0)
            .write_u8(2)
            .write_u32(0);
        w.write_zstr8(b"A").write_zstr8(b"");
        assert_eq!(read_use_point(w.as_bytes(), w.bit_len()), Some(2));
    }

    /// CLI-1250: the own stop starts only inside the point's box: the zone extent the server wrote (at least 96 on
    /// each axis), round the point.
    #[test]
    fn the_own_hold_stops_only_inside_the_points_box() {
        let zone = dc_dserver::world::ZoneInfo {
            kind: dc_dserver::world::ZoneKind::BombSite,
            pos: [1000.0, 0.0, 0.0],
            rot: [0.0; 3],
            dims: [200.0, 50.0, 120.0],
            name: b"A".to_vec(),
            team_text: None,
            number: 1,
            planted_command: Vec::new(),
        };
        let blob = dc_dserver::worldobj::zone_blob_as(&zone, 2, 2, 1);
        let half = read_zone_half(blob.as_bytes(), blob.bit_len()).unwrap();
        for (h, d) in half.iter().zip([200.0, 50.0, 120.0]) {
            assert!((h - d).abs() < 0.5, "{half:?}");
        }
        let mut objects = std::collections::BTreeMap::new();
        let mut o = dc_botclient::objects::ClientObject::default();
        o.create_pos = [1000.0, 0.0, 0.0];
        o.blob = Some(blob);
        objects.insert(7u16, o);
        assert!(hold_point_takes(&objects, 7, [1190.0, 90.0, -110.0]));
        assert!(!hold_point_takes(&objects, 7, [1210.0, 0.0, 0.0]), "outside x");
        assert!(!hold_point_takes(&objects, 7, [1000.0, 100.0, 0.0]), "outside y (96)");
        assert!(!hold_point_takes(&objects, 8, [1000.0, 0.0, 0.0]), "no such point");
    }
}
