//! The character sheet, its draft, and why a change can be refused.

/// The eight primary stats in `mes/stat.mes` order: ST DX CN BE IN PE WP CH.
pub const PRIMARY_STATS: usize = 8;
/// Every primary stat of a new character before race and background (template 016086).
pub const BASE_STAT: i32 = 8;
/// Points a new level-1 character starts with (stat 21 in the prototypes).
pub const STARTING_POINTS: i32 = 5;
/// Character points per step of a primary stat, hit points or fatigue (table 0x5b52e0: flat 1).
pub const STAT_COST: i32 = 1;
/// No stat goes below this (table 0x5b5194).
pub const MIN_STAT: i32 = 1;
/// Longest name: the original's buffer holds 23 bytes including the terminator (0x5461a0).
pub const MAX_NAME: usize = 22;
pub const RACE_COUNT: usize = 8;
/// Which races allow a female character (Arcanum.exe race table 0x5c5170).
pub const FEMALE_ALLOWED: [bool; RACE_COUNT] = [true, false, true, true, false, false, true, false];
pub const FEMALE: u8 = 0;
pub const MALE: u8 = 1;

/// The two pools bought with character points (+4 each per point).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Pool {
    HitPoints,
    Fatigue,
}

/// Changes since the last accept. Only these can be sold back ("cannot reduce below its last
/// accepted level", `mes/charedit.mes` 19).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Draft {
    pub stats: [i32; PRIMARY_STATS],
    pub hit_points: i32,
    pub fatigue: i32,
}

impl Draft {
    /// Character points the draft spends.
    pub fn spent(&self) -> i32 {
        (self.stats.iter().sum::<i32>() + self.hit_points + self.fatigue) * STAT_COST
    }
}

/// One character: accepted values plus a draft of unaccepted changes.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CharacterSheet {
    pub name: String,
    pub race: u8,
    pub gender: u8,
    pub age: i32,
    pub level: i32,
    /// `Rules/backgrnd.mes` block id (the block starts at `id * 10`).
    pub background: u32,
    /// `portrait/gameport.mes` id; `None` = the first portrait for the race and gender.
    pub portrait: Option<u32>,
    /// Accepted base stats, without race or background effects.
    pub base: [i32; PRIMARY_STATS],
    /// Accepted points in each pool.
    pub hp_points: i32,
    pub fatigue_points: i32,
    /// Accepted unspent character points.
    pub unspent: i32,
    pub draft: Draft,
}

impl Default for CharacterSheet {
    /// As the "create your own" template (prototype 016086): a human male, level 1, first
    /// portrait, background 0, no name, 5 points.
    fn default() -> Self {
        Self {
            name: String::new(),
            race: 0,
            gender: MALE,
            age: 0,
            level: 1,
            background: 0,
            portrait: None,
            base: [BASE_STAT; PRIMARY_STATS],
            hp_points: 0,
            fatigue_points: 0,
            unspent: STARTING_POINTS,
            draft: Draft::default(),
        }
    }
}

impl CharacterSheet {
    /// A name the player has actually entered.
    pub fn has_name(&self) -> bool {
        !self.name.trim().is_empty()
    }
}

/// Which text file a refusal's message is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Message {
    /// `mes/charedit.mes`
    Charedit(u32),
    /// `mes/MainMenu.mes`
    MainMenu(u32),
}

/// Why a change was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Refusal {
    /// charedit 17 "Stat has reached its maximum value."
    AtMaximum,
    /// charedit 18 "You haven't enough character points."
    NoPoints,
    /// charedit 19 "You cannot reduce this stat below its last accepted level."
    BelowAccepted,
    /// charedit 20 "Stat has reached its minimum value."
    AtMinimum,
    /// The original ignores the click without a message (e.g. female for a male-only race).
    NotAllowed,
    /// MainMenu 506: Next without a name.
    NoName,
    /// A pre-made character that is not in the data.
    UnknownPremade,
}

impl Refusal {
    /// The message the original shows, if any.
    pub fn message(self) -> Option<Message> {
        match self {
            Self::AtMaximum => Some(Message::Charedit(17)),
            Self::NoPoints => Some(Message::Charedit(18)),
            Self::BelowAccepted => Some(Message::Charedit(19)),
            Self::AtMinimum => Some(Message::Charedit(20)),
            Self::NoName => Some(Message::MainMenu(506)),
            Self::NotAllowed | Self::UnknownPremade => None,
        }
    }
}
