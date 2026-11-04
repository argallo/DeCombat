//! The scenarios of a run: which mode on which map, how many bots on which network profiles, for how long.
//!
//! Two kinds of run: with the user's ROM (`--rom` or `CA_ROM`) a scenario is a real room on a local lobby (the
//! lobby starts the game server, the host bot picks the mode and map); without it (`--direct`, CI) the runner starts
//! a game server on a synthetic world itself and the bots join it directly: no ROM content at all, the transport and
//! the session are the same.
// DC-DEFAULT: NET-22 (proposed) the network lab's matrices, see DC_DEFAULTS.md

use std::path::Path;

/// One group of bots on the same path.
#[derive(Debug, Clone, PartialEq)]
pub struct Group {
    /// `profile[,key=value...]` (dc_net_engine::impair::PathSpec::parse).
    pub net: String,
    pub count: usize,
    /// Seconds after the room's start (the first bot's) this group's bots log in; > 0 joins a running match.
    pub join_after_s: u32,
}

impl Group {
    pub fn new(net: &str, count: usize) -> Group {
        Group {
            net: net.to_string(),
            count,
            join_after_s: 0,
        }
    }
}

/// One room.
#[derive(Debug, Clone, PartialEq)]
pub struct Scenario {
    pub name: String,
    /// ROM mode row (`[Mode<n>]`), or `None` in a direct run.
    pub mode: Option<usize>,
    pub mode_key: String,
    /// ROM map row (`[Map<n>]`).
    pub map: Option<usize>,
    pub map_name: String,
    pub groups: Vec<Group>,
    /// Time in the match before the bots leave.
    pub minutes: f64,
    /// Bots of these profiles are expected to fail today (known open issues): their failures are reported as
    /// `xfail` and do not fail the run.
    pub xfail: Vec<String>,
}

impl Scenario {
    pub fn players(&self) -> usize {
        self.groups.iter().map(|g| g.count).sum()
    }
}

/// The mix of a full room: mostly good lines, every common bad one, one of each harsh one.
pub fn mixed16() -> Vec<Group> {
    vec![
        Group::new("good", 6),
        Group::new("wifi", 2),
        Group::new("mobile_hotspot", 2),
        Group::new("australia", 2),
        Group::new("lossy_burst", 2),
        Group::new("blackout", 1),
        Group::new("campus_firewall", 1),
    ]
}

/// Scales a mix to `players` (keeps every group with at least one bot while there is room; the first group, the
/// good lines, takes the rest).
pub fn scale(mix: &[Group], players: usize) -> Vec<Group> {
    let total: usize = mix.iter().map(|g| g.count).sum();
    if total == players || mix.is_empty() {
        return mix.to_vec();
    }
    let mut out: Vec<Group> = Vec::new();
    let mut left = players;
    for g in mix.iter().skip(1) {
        if left <= 1 {
            break;
        }
        let n = ((g.count * players) as f64 / total as f64).round().max(1.0) as usize;
        let n = n.min(left - 1);
        out.push(Group {
            count: n,
            ..g.clone()
        });
        left -= n;
    }
    out.insert(
        0,
        Group {
            count: left,
            ..mix[0].clone()
        },
    );
    out.retain(|g| g.count > 0);
    out
}

/// A playable mode of the ROM with the maps that play it.
#[derive(Debug, Clone, PartialEq)]
pub struct RomMode {
    pub index: usize,
    pub key: String,
    pub name: String,
    pub maps: Vec<(usize, String)>,
}

/// The ROM's playable modes (`EnablePlay`) and their maps (the room table's `GMS_<key>` entries).
pub fn rom_modes(rom: &Path) -> Result<Vec<RomMode>, String> {
    let g = dc_bute::GameRoomData::from_rom(rom).map_err(|e| format!("ROM room table: {e:?}"))?;
    let modes = g.modes(false);
    let maps = g.maps();
    let masks = g.map_mode_masks(false);
    let map_name = |i: usize| -> String {
        maps.get(i)
            .and_then(|m| {
                ["Name", "NameEng", "World", "Key"]
                    .iter()
                    .find_map(|k| m.get(k).cloned())
            })
            .unwrap_or_else(|| format!("map{i}"))
    };
    let mut out = Vec::new();
    for (i, m) in modes.iter().enumerate().take(32) {
        let enabled = m
            .get("EnablePlay")
            .is_none_or(|v| v.eq_ignore_ascii_case("TRUE"));
        if !enabled {
            continue;
        }
        let key = m.get("Key").cloned().unwrap_or_else(|| format!("mode{i}"));
        let name = ["Name", "NameEng"]
            .iter()
            .find_map(|k| m.get(k).cloned())
            .unwrap_or_else(|| key.clone());
        let maps: Vec<(usize, String)> = masks
            .iter()
            .filter(|(_, mask)| mask & (1 << i) != 0)
            .map(|(mi, _)| (*mi, map_name(*mi)))
            .collect();
        if !maps.is_empty() {
            out.push(RomMode {
                index: i,
                key,
                name,
                maps,
            });
        }
    }
    Ok(out)
}

/// The named matrices: `quick` (minutes), `soak` (long), `ci` (direct, no ROM).
pub const MATRICES: [&str; 4] = ["quick", "soak", "ci", "campus"];

/// Builds a matrix. `modes` is empty in a direct run.
pub fn build(
    name: &str,
    modes: &[RomMode],
    players: usize,
    minutes: Option<f64>,
) -> Result<Vec<Scenario>, String> {
    let direct = modes.is_empty();
    let mk =
        |sname: String, mode: Option<&RomMode>, map_pick: usize, groups: Vec<Group>, min: f64| {
            let (map, map_name) = match mode {
                Some(m) => {
                    let (i, n) = m.maps[map_pick % m.maps.len()].clone();
                    (Some(i), n)
                }
                None => (None, "synthetic".to_string()),
            };
            Scenario {
                name: sname,
                mode: mode.map(|m| m.index),
                mode_key: mode.map(|m| m.key.clone()).unwrap_or_else(|| "TDM".into()),
                map,
                map_name,
                groups: scale(&groups, players),
                minutes: minutes.unwrap_or(min),
                xfail: Vec::new(),
            }
        };
    let mut out = Vec::new();
    match name {
        // every mode once on its own map, a full room of mixed paths; then the path-wide trouble makers together
        "quick" => {
            if direct {
                out.push(mk("mixed".into(), None, 0, mixed16(), 3.0));
            }
            for (k, m) in modes.iter().enumerate() {
                out.push(mk(
                    format!("{}-mixed", m.key.to_ascii_lowercase()),
                    Some(m),
                    k,
                    mixed16(),
                    2.0,
                ));
            }
            out.push(mk(
                "nat-and-firewall".into(),
                modes.first(),
                0,
                vec![
                    Group::new("good", 10),
                    Group::new("cgnat_rebind", 2),
                    Group::new("campus_firewall", 2),
                    Group::new("blackout", 2),
                ],
                3.0,
            ));
        }
        // every mode on up to three maps, longer, then a long mixed room
        "soak" => {
            for m in modes {
                for pick in 0..m.maps.len().min(3) {
                    out.push(mk(
                        format!("{}-{}", m.key.to_ascii_lowercase(), pick),
                        Some(m),
                        pick,
                        mixed16(),
                        6.0,
                    ));
                }
            }
            out.push(mk("long-mixed".into(), modes.first(), 0, mixed16(), 30.0));
        }
        // CI: no ROM; the transport over each kind of path, the room filled from the start and by a late wave
        "ci" => {
            out.push(mk("mixed".into(), None, 0, mixed16(), 2.0));
            let mut late = vec![Group::new("campus_firewall", 1), Group::new("good", 8)];
            late[1].join_after_s = 40;
            out.push(mk("late-wave".into(), None, 0, late, 2.0));
            out.push(mk(
                "harsh".into(),
                None,
                0,
                vec![
                    Group::new("lossy_burst", 4),
                    Group::new("mobile_hotspot", 4),
                    Group::new("blackout", 2),
                ],
                2.0,
            ));
        }
        // the 2026-11-02 report: one player on a campus line, then 7 more join the running match
        "campus" => {
            let mut groups = vec![
                Group::new("campus_firewall", 1),
                Group::new("good", 1),
                Group::new("good", 7),
            ];
            groups[2].join_after_s = 45;
            let mut s = mk("campus".into(), modes.first(), 0, groups.clone(), 4.0);
            // the report's shape, not a full room: 2 + 7 (scaled down only for a smaller --players)
            if players >= 9 {
                s.groups = groups;
            }
            s.groups.retain(|g| g.count > 0);
            out.push(s);
        }
        other => {
            return Err(format!(
                "unknown matrix {other:?} (known: {})",
                MATRICES.join(", ")
            ))
        }
    }
    Ok(out)
}

/// `--mix good:6,wifi:2,...` (a `+`-separated override after the profile: `wifi+loss=20:2`).
pub fn parse_mix(text: &str) -> Result<Vec<Group>, String> {
    let mut out = Vec::new();
    for part in text
        .split(';')
        .flat_map(|p| p.split(' '))
        .filter(|p| !p.is_empty())
    {
        for item in part.split(',').filter(|s| !s.is_empty()) {
            let (net, n) = item.rsplit_once(':').unwrap_or((item, "1"));
            let count: usize = n
                .parse()
                .map_err(|_| format!("--mix: bad count in {item:?}"))?;
            let net = net.replace('+', ",");
            dc_net_engine::impair::PathSpec::parse(&net)?;
            out.push(Group::new(&net, count));
        }
    }
    if out.is_empty() {
        return Err("--mix: empty".into());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaling_keeps_every_group_and_the_total() {
        let m = mixed16();
        assert_eq!(scale(&m, 16), m);
        for n in [4, 8, 10, 24] {
            let s = scale(&m, n);
            assert_eq!(s.iter().map(|g| g.count).sum::<usize>(), n, "{n}");
            assert_eq!(s[0].net, "good");
        }
    }

    #[test]
    fn direct_matrices_need_no_rom_and_every_profile_parses() {
        for name in MATRICES {
            let sc = build(name, &[], 16, None).unwrap();
            assert!(!sc.is_empty(), "{name}");
            for s in sc {
                for g in &s.groups {
                    dc_net_engine::impair::PathSpec::parse(&g.net).unwrap();
                }
            }
        }
        assert!(build("nope", &[], 16, None).is_err());
    }

    #[test]
    fn mix_text() {
        let m = parse_mix("good:3,wifi+loss=20:2,australia").unwrap();
        assert_eq!(m.len(), 3);
        assert_eq!(m[1].net, "wifi,loss=20");
        assert_eq!(m[2].count, 1);
        assert!(parse_mix("nope:2").is_err());
    }
}
