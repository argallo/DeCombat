//! CLI-1955: the sound mixer: the effects volume broken down into groups the player sets one by one (the Sound
//! tab's "Sound Mixer" page). Every sound the client starts belongs to one group by what started it; the mixer
//! scales each group's voices by its volume on top of the effects volume.
//!
//! Provenance: ORIGINAL (DeCombat's own setting; the 2009 client has one effects volume).
// DC-DEFAULT: CLI-1955 (proposed, owner request 2026-11-09 "break down sound effects master volume into a bunch of
// it's various parts, people have mentioned certain sounds being too loud"), see DC_DEFAULTS.md

use crate::sound::Trigger;

/// One group of the mixer. The number is the mixer's group (0 is none).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SoundGroup {
    OwnFire = 1,
    OwnHandling,
    OthersFire,
    OthersHandling,
    Flyby,
    HitMarker,
    OwnSteps,
    OthersSteps,
    Voices,
    Breath,
    World,
    Announcer,
    Menu,
    /// Bullet impacts (CLI-1961): a group of their own for the voice cap and the trim; the page has no row for it,
    /// its volume is the "Explosions / Map" slider's ([`SoundGroup::slider`]).
    Impacts,
}

/// The groups in the page's order.
pub const GROUPS: [SoundGroup; 13] = [
    SoundGroup::OwnFire,
    SoundGroup::OwnHandling,
    SoundGroup::OthersFire,
    SoundGroup::OthersHandling,
    SoundGroup::Flyby,
    SoundGroup::HitMarker,
    SoundGroup::OwnSteps,
    SoundGroup::OthersSteps,
    SoundGroup::Voices,
    SoundGroup::Breath,
    SoundGroup::World,
    SoundGroup::Announcer,
    SoundGroup::Menu,
];

/// Every group the mixer knows: the page's [`GROUPS`] and the ones that follow a page slider.
pub const ALL_GROUPS: [SoundGroup; 14] = [
    SoundGroup::OwnFire,
    SoundGroup::OwnHandling,
    SoundGroup::OthersFire,
    SoundGroup::OthersHandling,
    SoundGroup::Flyby,
    SoundGroup::HitMarker,
    SoundGroup::OwnSteps,
    SoundGroup::OthersSteps,
    SoundGroup::Voices,
    SoundGroup::Breath,
    SoundGroup::World,
    SoundGroup::Announcer,
    SoundGroup::Menu,
    SoundGroup::Impacts,
];

/// Whether the voice caps ([`SoundGroup::default_cap`]) are on when nothing set them: off, so the mix is as before
/// until the owner decides (CLI-1961).
// DC-DEFAULT: CLI-1961 (proposed)
pub const VOICE_CAPS_DEFAULT: bool = false;

/// A group's volume when the player never set it.
pub const DEFAULT_VOLUME: u8 = 100;

/// The volumes of all groups, in [`GROUPS`] order (0..=100).
pub type Mix = [u8; GROUPS.len()];

pub const DEFAULT_MIX: Mix = [DEFAULT_VOLUME; GROUPS.len()];

impl SoundGroup {
    /// The mixer's group number.
    pub fn id(self) -> u8 {
        self as u8
    }

    /// The index in [`GROUPS`] (and in a [`Mix`]) of the group's slider.
    pub fn index(self) -> usize {
        self.slider() as usize - 1
    }

    /// The page row whose slider sets this group's volume: itself, or for a group without a row the row it was
    /// part of before it had a group of its own (the impacts: "Explosions / Map").
    pub fn slider(self) -> SoundGroup {
        match self {
            SoundGroup::Impacts => SoundGroup::World,
            g => g,
        }
    }

    /// The most voices of the group that play at once when the caps are on (CLI-1961). The player's own fire,
    /// handling and steps, the voices, the announcer and the menu are never capped (the player's own bullet impacts
    /// share the impacts group with everyone's) (a fire or step sound of one emitter already cuts
    /// its previous one); the others are sized for a 16-player room, so an eight-player match reaches only the
    /// impacts' and the flybys' caps, and then loses the quietest (farthest) ones.
    // DC-DEFAULT: CLI-1961 (proposed)
    pub fn default_cap(self) -> Option<u16> {
        match self {
            SoundGroup::OthersFire => Some(24),
            SoundGroup::OthersHandling => Some(16),
            SoundGroup::Flyby => Some(8),
            SoundGroup::OthersSteps => Some(24),
            SoundGroup::World => Some(24),
            SoundGroup::Impacts => Some(32),
            SoundGroup::OwnFire
            | SoundGroup::OwnHandling
            | SoundGroup::HitMarker
            | SoundGroup::OwnSteps
            | SoundGroup::Voices
            | SoundGroup::Breath
            | SoundGroup::Announcer
            | SoundGroup::Menu => None,
        }
    }

    /// The key in the profile and on the page (lower case, digits and `_`).
    pub fn key(self) -> &'static str {
        match self {
            SoundGroup::OwnFire => "own_fire",
            SoundGroup::OwnHandling => "own_handling",
            SoundGroup::OthersFire => "others_fire",
            SoundGroup::OthersHandling => "others_handling",
            SoundGroup::Flyby => "flyby",
            SoundGroup::HitMarker => "hit_marker",
            SoundGroup::OwnSteps => "own_steps",
            SoundGroup::OthersSteps => "others_steps",
            SoundGroup::Voices => "voices",
            SoundGroup::Breath => "breath",
            SoundGroup::World => "world",
            SoundGroup::Announcer => "announcer",
            SoundGroup::Menu => "menu",
            SoundGroup::Impacts => "impacts",
        }
    }

    /// The row's label on the page.
    pub fn label(self) -> &'static str {
        match self {
            SoundGroup::OwnFire => "Your Gunshots",
            SoundGroup::OwnHandling => "Your Reloads / Swaps",
            SoundGroup::OthersFire => "Others' Gunshots",
            SoundGroup::OthersHandling => "Others' Reloads",
            SoundGroup::Flyby => "Bullet Flybys",
            SoundGroup::HitMarker => "Hit Markers",
            SoundGroup::OwnSteps => "Your Steps / Jumps",
            SoundGroup::OthersSteps => "Others' Steps / Jumps",
            SoundGroup::Voices => "Voices / Callouts",
            SoundGroup::Breath => "Sprint Breathing",
            SoundGroup::World => "Explosions / Map",
            SoundGroup::Announcer => "Announcer / HUD",
            SoundGroup::Menu => "Menu / Lobby",
            SoundGroup::Impacts => "Bullet Impacts",
        }
    }

    /// The heading the page shows the row under.
    pub fn heading(self) -> &'static str {
        match self {
            SoundGroup::OwnFire
            | SoundGroup::OwnHandling
            | SoundGroup::OthersFire
            | SoundGroup::OthersHandling
            | SoundGroup::Flyby
            | SoundGroup::HitMarker => "Weapons",
            SoundGroup::OwnSteps
            | SoundGroup::OthersSteps
            | SoundGroup::Voices
            | SoundGroup::Breath => "Players",
            SoundGroup::World | SoundGroup::Impacts => "World",
            SoundGroup::Announcer | SoundGroup::Menu => "Interface",
        }
    }

    /// The group of a sound by what started it; `menu`: the front end played it (clicks, the case reel's gun).
    pub fn of(trigger: &Trigger, menu: bool) -> SoundGroup {
        if menu {
            return SoundGroup::Menu;
        }
        match trigger {
            Trigger::OwnFire => SoundGroup::OwnFire,
            Trigger::OwnWeapon { .. } => SoundGroup::OwnHandling,
            Trigger::ShotFire { .. } | Trigger::ShotSilenced { .. } => SoundGroup::OthersFire,
            Trigger::SoundRelay { k, .. } if *k == dc_botclient::fx::SOUND_JUMP => {
                SoundGroup::OthersSteps
            }
            Trigger::SoundRelay { .. } => SoundGroup::OthersHandling,
            Trigger::Flyby { .. } => SoundGroup::Flyby,
            Trigger::OwnHit { .. } => SoundGroup::HitMarker,
            Trigger::Footstep { own: true, .. }
            | Trigger::OwnStep { .. }
            | Trigger::JumpStep
            | Trigger::JumpSound => SoundGroup::OwnSteps,
            Trigger::Footstep { .. } => SoundGroup::OthersSteps,
            Trigger::OwnKill | Trigger::Callout { .. } => SoundGroup::Voices,
            Trigger::Breath => SoundGroup::Breath,
            Trigger::LoadSound { .. } | Trigger::TrackedSound { .. } | Trigger::OneShot => {
                SoundGroup::World
            }
            Trigger::Impact => SoundGroup::Impacts,
            // the front end's own sounds (clicks, its boxes, the result jingles) keep their names when the options
            // over a match play them with the match's sounds
            Trigger::Interface { what } if is_menu_cue(what) => SoundGroup::Menu,
            Trigger::Interface { .. } => SoundGroup::Announcer,
        }
    }

    pub fn of_key(key: &str) -> Option<SoundGroup> {
        GROUPS.iter().copied().find(|g| g.key() == key)
    }
}

/// An interface sound of the front end's table (its name, or the name of an id it does not know).
fn is_menu_cue(what: &str) -> bool {
    what == crate::menu::frontfx::cue_name(i32::MIN)
        || crate::menu::frontfx::SOUND_KEYS
            .iter()
            .any(|(_, n, _)| *n == what)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_are_numbered_in_page_order_with_unique_keys() {
        for (i, g) in GROUPS.iter().enumerate() {
            assert_eq!(g.index(), i);
            assert_eq!(g.id() as usize, i + 1);
            assert!((g.id() as usize) < dc_audio::mixer::GROUP_COUNT);
            assert_eq!(SoundGroup::of_key(g.key()), Some(*g));
            assert!(g.key().bytes().all(|b| b.is_ascii_lowercase() || b == b'_'));
        }
    }

    #[test]
    fn every_group_has_a_mixer_number_and_the_impacts_follow_the_world_slider() {
        // CLI-1961: the impacts have their own group (cap, trim) but no row: the page is unchanged
        assert_eq!(GROUPS.len(), 13);
        assert!(!GROUPS.contains(&SoundGroup::Impacts));
        for (i, g) in ALL_GROUPS.iter().enumerate() {
            assert_eq!(g.id() as usize, i + 1);
            assert!((g.id() as usize) < dc_audio::mixer::GROUP_COUNT);
        }
        assert_eq!(SoundGroup::Impacts.slider(), SoundGroup::World);
        assert_eq!(SoundGroup::Impacts.index(), SoundGroup::World.index());
        assert_eq!(SoundGroup::of_key("impacts"), None, "not a stored key");
        // the player's own sounds are never capped
        for g in [
            SoundGroup::OwnFire,
            SoundGroup::OwnHandling,
            SoundGroup::OwnSteps,
        ] {
            assert_eq!(g.default_cap(), None);
        }
        assert!(SoundGroup::Impacts.default_cap().is_some());
    }

    #[test]
    fn each_sound_lands_in_the_group_a_player_would_look_for() {
        let of = |t: Trigger| SoundGroup::of(&t, false);
        assert_eq!(of(Trigger::OwnFire), SoundGroup::OwnFire);
        assert_eq!(of(Trigger::OwnWeapon { k: 3 }), SoundGroup::OwnHandling);
        assert_eq!(of(Trigger::ShotFire { shooter: 4 }), SoundGroup::OthersFire);
        assert_eq!(
            of(Trigger::ShotSilenced {
                shooter: 4,
                level: 2
            }),
            SoundGroup::OthersFire
        );
        assert_eq!(
            of(Trigger::SoundRelay { k: 3, player: 9 }),
            SoundGroup::OthersHandling
        );
        assert_eq!(
            of(Trigger::SoundRelay {
                k: dc_botclient::fx::SOUND_JUMP,
                player: 9
            }),
            SoundGroup::OthersSteps
        );
        assert_eq!(of(Trigger::OwnHit { first: 2 }), SoundGroup::HitMarker);
        assert_eq!(
            of(Trigger::OwnStep {
                left: true,
                surface: 0
            }),
            SoundGroup::OwnSteps
        );
        assert_eq!(of(Trigger::Breath), SoundGroup::Breath);
        assert_eq!(of(Trigger::OneShot), SoundGroup::World);
        assert_eq!(of(Trigger::Impact), SoundGroup::Impacts);
        assert_eq!(
            of(Trigger::Interface { what: "ping_place" }),
            SoundGroup::Announcer
        );
        assert_eq!(
            SoundGroup::of(&Trigger::Interface { what: "select" }, true),
            SoundGroup::Menu
        );
        // a menu click of the options over a match comes with the match's sounds
        assert_eq!(of(Trigger::Interface { what: "select" }), SoundGroup::Menu);
        assert_eq!(
            of(Trigger::Interface { what: "head_shot" }),
            SoundGroup::Announcer
        );
        assert_eq!(
            SoundGroup::of(&Trigger::OwnWeapon { k: 6 }, true),
            SoundGroup::Menu
        );
    }
}
