//! The least gap between two shots of a weapon, from the weapon's own first-person animation files: the bot's mirror
//! of the client's shot timing (`dc_client::arms::Arsenal`, Phase::Fire / Phase::PostFire; dc-client depends on this
//! crate, so the rule is repeated here, not shared).
//!
//! The client has no table fire delay: a shot is a marker of the fire clip, the next shot of a held trigger starts
//! where the fire clip ended (an automatic weapon: the cadence is the fire clip length), and a one-press-per-shot
//! weapon plays its postfire clip after the fire clip before the next press is taken.

use dc_rez::RezVfs;

// OBSERVED: the first-person clips are matched by the first 8 bytes of the SHA-256 of the lower-cased clip name
// (same keys as dc_client::arms::CLIP_FIRE / CLIP_POSTFIRE; the names are not stored)
const CLIP_FIRE: [u8; 8] = [0xe5, 0x5d, 0x4d, 0xe3, 0x10, 0x36, 0x4c, 0x9d];
const CLIP_FIRE_LEN: usize = 6;
const CLIP_POSTFIRE: [u8; 8] = [0xd7, 0x8d, 0xee, 0x09, 0x45, 0x91, 0x74, 0x4b];
const CLIP_POSTFIRE_LEN: usize = 8;

/// The fire and postfire clip lengths (ms) of a composed clip list (own clips, then each child's); first match wins.
pub fn clip_lengths<'a>(
    clips: impl IntoIterator<Item = &'a dc_model::Clip>,
) -> (Option<u32>, Option<u32>) {
    let (mut fire, mut post) = (None, None);
    for c in clips {
        let name = c.name.as_bytes().to_ascii_lowercase();
        let h = dc_rom::sha256(&name);
        let len = c.markers.iter().map(|m| m.time_ms).max().unwrap_or(0);
        if h[..8] == CLIP_FIRE && name.len() == CLIP_FIRE_LEN {
            fire.get_or_insert(len);
        } else if h[..8] == CLIP_POSTFIRE && name.len() == CLIP_POSTFIRE_LEN {
            post.get_or_insert(len);
        }
    }
    (fire, post)
}

/// The least gap between shots: an automatic row (`RapidFireCount` 0 or absent) repeats at the fire clip; a
/// one-press-per-shot row (bolt, semi) needs the fire clip plus the postfire clip. `None` = no fire clip known.
// DC-DEFAULT: BOT-60 (proposed) the bot never shoots faster than the weapon's own animation cycle; `--fire-interval`
// is the floor
pub fn cycle_ms(single_shot: bool, fire: Option<u32>, post: Option<u32>) -> Option<u32> {
    let fire = fire?;
    Some(if single_shot {
        fire + post.unwrap_or(0)
    } else {
        fire
    })
}

/// A ROM path `Dir\file` -> (archive, entry) as the client resolves it.
fn rez_entry_of(path: &str) -> Option<(String, String)> {
    let p = path.trim().replace('/', "\\");
    let p = p.trim_start_matches('\\');
    let (top, rest) = p.split_once('\\')?;
    if top.is_empty() || rest.is_empty() {
        return None;
    }
    Some((format!("{top}.rez"), p.to_string()))
}

fn child_path(parent: &str, child: &str) -> String {
    let child = child.replace('/', "\\");
    if child.contains('\\') {
        return child;
    }
    match parent.replace('/', "\\").rsplit_once('\\') {
        Some((folder, _)) => format!("{folder}\\{child}"),
        None => child,
    }
}

fn read(vfs: &RezVfs, path: &str) -> Option<Vec<u8>> {
    let (rez, entry) = rez_entry_of(path)?;
    vfs.entry(&rez, &entry).ok().flatten()
}

/// The fire and postfire clip lengths of the first of `paths` (first-person models) that the ROM holds.
pub fn model_clip_lengths(vfs: &RezVfs, paths: &[String]) -> (Option<u32>, Option<u32>) {
    for p in paths {
        let Some(bytes) = read(vfs, p) else { continue };
        let Ok(body) = dc_model::Model::parse(&bytes) else {
            continue;
        };
        let mut all: Vec<dc_model::Clip> = body.clips.clone();
        for c in &body.child_models {
            let cp = child_path(p, &c.to_text());
            if let Some(cm) = read(vfs, &cp).and_then(|b| dc_model::Model::parse(&b).ok()) {
                all.extend(cm.clips);
            }
        }
        return clip_lengths(all.iter());
    }
    (None, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_shot_rows_add_the_postfire_clip() {
        assert_eq!(cycle_ms(true, Some(120), Some(550)), Some(670));
        assert_eq!(cycle_ms(false, Some(105), Some(0)), Some(105));
        assert_eq!(cycle_ms(false, Some(105), Some(300)), Some(105));
        assert_eq!(cycle_ms(true, None, Some(5)), None);
    }
}
