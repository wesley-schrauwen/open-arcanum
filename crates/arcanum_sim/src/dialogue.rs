//! Conversations with NPCs, run by the host (Arcanum.exe: talk 0x42af00 → 0x57cc70, start
//! 0x567460, the dialogue module 0x412e10-0x419e70). See `plans/dialogue.md`.
//!
//! - **Talking** ([`PlayerIntent::Talk`](crate::PlayerIntent::Talk)): the PC walks to the NPC;
//!   within [`crate::USE_RANGE`] the NPC's dialogue script (event [`EVENT_DIALOGUE`]) runs. Its
//!   action 4 starts a conversation in the script's own dialogue file ([`StartedDialogues`]);
//!   any script can start one the same way.
//! - **A line** (0x416840): the NPC's text for the PC's gender, names put in (`@pcname@`,
//!   `@npcname@`), and its result run. It floats over the NPC for every player
//!   ([`LedgerDelta::FloatText`](crate::LedgerDelta::FloatText)); its voice file goes with it.
//! - **Options** (0x414f50): the PC options after the line up to the next NPC line that pass
//!   gender, intelligence and the test; at most [`MAX_OPTIONS`]. Only the talker gets them
//!   ([`LedgerDelta::DialogueOptions`](crate::LedgerDelta::DialogueOptions)). None: the
//!   conversation ends.
//! - **Choosing** ([`PlayerIntent::Choose`](crate::PlayerIntent::Choose)): the option floats
//!   over the PC (green), its result runs, then its response: a line, 0 the end, `-n` line `n`
//!   (an NPC line is shown; a PC option is taken as chosen, without showing it: builder's
//!   reading of the undecoded state 2).
//! - **Ending** ([`PlayerIntent::EndTalk`](crate::PlayerIntent::EndTalk), the response 0, no
//!   options): the talker gets [`LedgerDelta::DialogueEnded`](crate::LedgerDelta::DialogueEnded);
//!   the NPC has now met the PC (test `me`).
//!
//! **Built tests:** `gv gf pv pf lf lc qu qa qb ss ru rq rp me $$ in ni ch pe le ra ps ha`.
//! **Built results:** `$$ gv gf pv pf lf lc qu ss ru rq rp fl`. The NPC's flags and counters
//! (`lf`, `lc`) are its dialogue attachment's (event 9, "scope 9"). Item numbers are basic
//! prototype numbers (+20, as `InvenSource`; to confirm). A test the host cannot check yet fails;
//! a result it cannot do yet is skipped; both are reported ([`DialogueReports`]).
//!
//! **Generic lines** come from the `mes/gd_*.mes` files (table 0x5a063c), picked at random in a
//! block (0x4d4550 counts the lines in `lo..=hi`, the pick is `lo + r`). Files go by the NPC's and
//! the PC's genders (`m2f`: a male NPC to a female PC), and their `dumb` twins for a speaker of
//! Intelligence 4 or less (0x45fc00). Before a generic NPC line the NPC's override dialogue (its
//! script attachment [`EVENT_OVERRIDE`], 0x418870) is asked for a fixed line number; that line
//! carries a voice, generic ones do not.
//! - **`G:` NPC lines** ([`Say::greeting`], 0x419260): `G: k line, k fl line, ...` with `k` 0-9.
//!   `k` 0-8 are special cases (dead, bad associates, invisible, body spell, polymorph, shrunk,
//!   underwear, barbarian armour), none built. `k` 9 is the greeting: a line number goes to that
//!   line, `fl n` closes with line `n`, `0` says nothing. Else the greeting is by meeting and
//!   reaction level (0x4c0fc0): first meeting `gd_cls` 8000 / 9000 / 10000 for levels 0 / 1-2 / 3,
//!   `gd_rce` 2000 / 3000 for 4-5 / 6; met before, 11000 / 12000 / 13000 and 4000 / 5000. The
//!   options after the `G:` line follow.
//! - **`I:` NPC lines**: an insult, `gd_rce` 1000 (0x4185f0); options follow.
//! - **Class blocks** (`gd_cls`) are 50 lines per `npc_social_class` (the dumb file has one block);
//!   **race blocks** (`gd_rce`) 50 per PC race, the first for the NPC's own race (dumb: one).
//! - **Option texts** (0x416c10), to the NPC's gender: `Y: N: S: E: F: K: W:` from `gd_pc2`;
//!   `A:` thanks (`gd_cls_pc2` 1000) and `I:` an insult (`gd_cls_pc2` 3000), by the NPC's class;
//!   `C:` the story question (`gd_pc2` 1700 + story state).
//! - **Choosing** (0x413130): the result runs first. `I:` lowers the NPC's reaction by 10
//!   (0x4191e0); below 21 the NPC insults back and the talk ends. `C:` (0x41a3e0): the NPC answers
//!   from `gd_sto` (its race + 100 × story state), and the PC gets one "thanks" option that goes
//!   on to the option's response.
//! - **No options:** the PC gets a goodbye (`gd_pc2` 400-499) that ends the talk (0x414e60).
//! - **Barks** (0x5681c0 → 0x4132a0): when the NPC's dialogue script runs the default (or it has
//!   none), the NPC says the greeting over its head, and there is no conversation.
//!
//! - **`B:`** (barter, 300-399): the talk ends and barter opens ([`crate::barter`]); its result is
//!   not run.
//!
//! - **Services** (`plans/dialogue.md`): `R:` rumours, `T:` training to Apprentice, `Q:` quest
//!   lines, and the price dialogue they share.
//!
//! - **Directions** (`D:` / `X:`, [`crate::areas`]): a list of unknown areas, then the way there.
//!
//! - **Newspapers** (`P:`, `plans/dialogue.md` 10): today's headline, older papers, buying one.
//!
//! Not built: the other prefixes (`L: Z:`; left out and reported); the
//! `G:` special cases and the reputation greetings (0x4c1f80, one in five); the NPC's can-talk
//! checks (0x4ad800) and hostility at choice time (0x4ae120); alignment, aptitude, training and the
//! rest of the tests and results; turning to face each other.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use arcanum_formats::dialog::{DialogFile, DialogLine, Token, effects, tests};
use arcanum_formats::mes::MesFile;
use arcanum_rules::{CharacterSheet, FEMALE, RulesData};
use bevy_ecs::prelude::*;
use serde::{Deserialize, Serialize};

use crate::{Attachment, Inventory, LedgerDelta, LoadedMap, NetId, Outbox, PcScriptState, PlayerControlled, PlayerId, Position, ScriptGlobals, ScriptObject, Session, Sheet, SimRules, WorldState};

/// The dialogue event (attach point 9).
pub const EVENT_DIALOGUE: u32 = 9;
/// Experience for quests by level (0x4c53c0).
pub const XP_QUEST: &str = "Rules/xp_quest.mes";
/// The attach point whose script names the NPC's override dialogue (0x418870 reads
/// `scripts_idx` entry 0x1f).
pub const EVENT_OVERRIDE: u32 = 31;
/// Most options shown.
pub const MAX_OPTIONS: usize = arcanum_formats::dialog::MAX_OPTIONS;
/// How far `-n` responses may chain.
const MAX_JUMPS: usize = 8;

/// Which map object an NPC is: its place in its sector's mobile objects.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct NpcSource {
    pub sector: arcanum_formats::map::SectorId,
    pub index: u32,
}

impl NpcSource {
    pub fn object(self) -> ScriptObject {
        ScriptObject::Mobile { sector: self.sector, index: self.index }
    }
}

/// A conversation a script started: the talker, the other party, the file and the line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StartedDialogue {
    pub player: PlayerId,
    pub npc: ScriptObject,
    pub dialog: u32,
    pub line: i32,
}

/// Conversations started this tick, waiting for [`run_conversations`].
#[derive(Resource, Debug, Default)]
pub struct StartedDialogues(pub Vec<StartedDialogue>);

/// A PC's conversation: with whom, in which file, the NPC line shown and the options offered
/// (their lines and texts).
#[derive(Component, Debug, Clone, PartialEq, Eq)]
pub struct Conversation {
    pub npc: NetId,
    pub npc_object: ScriptObject,
    pub dialog: u32,
    pub line: i32,
    pub options: Vec<Offered>,
}

/// An option offered: its text and what choosing it does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offered {
    pub text: String,
    pub act: OptionAct,
}

/// What an offered option does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptionAct {
    /// A PC option of the file: its result, then its response.
    Line(i32),
    /// A generic option: this response only (0 ends).
    Respond(i32),
    /// A skill of a `T:` list (action 6, 0x418de0); `ret` is the `T:` option's response.
    Train { skill: usize, ret: i32 },
    /// Accepting a price (action 4, 0x418b30): pay, then the service.
    Pay { price: i32, service: Service, ret: i32 },
    /// An area of a directions list (actions 0x13 / 0x16).
    Direct { area: u32, price: i32, mark: bool, ret: i32 },
    /// The next page of the directions list of option line `option` (actions 0x12 / 0x15).
    MoreDirections { option: i32, offset: usize, ret: i32 },
    /// A page of the healing menu (actions 0xb-0xd: 0 all, 1 healing spells, 2 poison).
    HealPage { page: u8, ret: i32 },
    /// A healing chosen (actions 0xe / 0xf): free from a follower, else priced.
    Heal { healing: Healing, ret: i32 },
    /// Today's headline read out (action 0x1a, 0x41a620).
    Headline { ret: i32 },
    /// The older papers listed (action 0x1b, 0x41a700).
    OldPapers { ret: i32 },
    /// A paper chosen: its price asked (action 0x1c, 0x41a880).
    BuyPaper { paper: i32, ret: i32 },
}

/// A newspaper's price before the reaction factor (0x418a40 with 2).
pub const NEWSPAPER_PRICE: i32 = 2;
/// The newspaper item: basic prototype 14064 (0x41a8c0).
pub const NEWSPAPER_PROTOTYPE: u32 = 14064 + crate::BASIC_TO_PROTOTYPE;

/// What an NPC is paid for (the price dialogue's stored action).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Service {
    /// Telling rumour `n` (action 9, 0x4190e0).
    Rumour(u32),
    /// Training skill `k` to Apprentice (action 7, 0x418f30).
    Train(usize),
    /// Directions to an area, and with `mark` marking it on the map (0x41a150 / 0x41a290).
    Directions { area: u32, mark: bool },
    /// Healing with a skill or a spell (actions 0x10 / 0x11, 0x419cb0 / 0x419d50).
    Heal(Healing),
    /// Newspaper `n` (action 0x1d, 0x41a8c0).
    Newspaper(i32),
}

/// What an NPC heals the PC with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Healing {
    /// Skill N used on the PC (`U:N`, 10 Heal).
    Skill(usize),
    /// A white necromancy spell cast on the PC (60-64).
    Spell(u32),
}

/// What an NPC can heal with (0x419830 / 0x419a00).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Healer {
    /// Spells known in college 12, white necromancy (field 0xdf `critter_spell_tech_idx` 12).
    pub college: i32,
    /// Skills as stored (fields 0xdd / 0xde: points in bits 0-5, training rank in bits 6-7),
    /// basic then tech.
    pub skills: [i32; arcanum_rules::SKILLS],
    /// Carries a healing kit: a generic item with `generic_flags` 8 (0x4c91f0 / 0x4ae570).
    pub kit: bool,
    /// The magick/tech aptitude (stat 0x10) from its spells and tech degrees.
    pub aptitude: i32,
}

impl Healer {
    /// Skill `k`'s level (0x4c5eb0: points × 4, at most 20; the NPC's stats and effects do not
    /// cap or move it here) and rank (0x4c60c0).
    pub fn skill(&self, k: usize) -> (i32, i32) {
        let raw = self.skills.get(k).copied().unwrap_or(0);
        (((raw & 0x3f) * arcanum_rules::LEVELS_PER_POINT).min(arcanum_rules::MAX_SKILL_LEVEL), (raw >> 6) & 3)
    }
}

/// The Heal skill.
pub const HEAL_SKILL: usize = 10;
/// The healing spells (`mes/spell.mes`): Minor Healing, Halt Poison, Major Healing, Resurrect.
pub const MINOR_HEALING: u32 = 60;
pub const HALT_POISON: u32 = 61;
pub const MAJOR_HEALING: u32 = 62;
pub const RESURRECT: u32 = 64;
/// What a spell costs before the reaction factor (0x4b1740).
pub const SPELL_PRICE: i32 = 100;

/// Hit points a healing spell restores for a caster of `aptitude` (`Rules/SpellList.mes`
/// "Heal Dmg lo-hi, Scaled", 0x4521a0: `lo + (hi - lo) × aptitude / 100` above 0, else `lo`).
pub fn spell_heal(spell: u32, aptitude: i32) -> i32 {
    let (lo, hi) = match spell {
        MINOR_HEALING => (5, 30),
        MAJOR_HEALING => (40, 80),
        _ => return 0,
    };
    if aptitude > 0 { lo + (hi - lo) * aptitude / 100 } else { lo }
}

/// The Apprentice rank: the only one an NPC trains (0x418f30 grants rank 1).
pub const APPRENTICE: u8 = 1;
/// The skill level each training rank needs (table 0x5b6f44).
pub const RANK_LEVELS: [i32; 4] = [0, 1, 9, 18];
/// What training costs before the reaction factor (0x418de0).
pub const TRAINING_PRICE: i32 = 100;
/// The quest line table (0x4c4860).
pub const GAME_QUEST: &str = "Rules/gamequest.mes";

/// A list of numbers and ranges, comma separated (0x417f90): `9, 11` or `1040-1049`; at most 100.
pub fn number_list(text: &str) -> Vec<i32> {
    use arcanum_formats::dialog::atoi;
    let mut out = Vec::new();
    for item in text.split(',') {
        let item = item.trim();
        if item.is_empty() {
            continue;
        }
        match item[1..].find('-').map(|i| i + 1) {
            Some(dash) => out.extend(atoi(item[..dash].trim().as_bytes())..=atoi(item[dash + 1..].trim().as_bytes())),
            None => out.push(atoi(item.as_bytes())),
        }
        if out.len() >= 100 {
            out.truncate(100);
            break;
        }
    }
    out
}

/// A service option's arguments (the text after the prefix, 0x416c10): `$price, list`, brackets
/// allowed. The price is 0 without a `$`.
pub fn priced_list(args: &str) -> (i32, Vec<i32>) {
    let args = args.trim().trim_start_matches('(').trim_end_matches(')').trim();
    match args.strip_prefix('$') {
        Some(rest) => {
            let (price, list) = rest.split_once(',').unwrap_or((rest, ""));
            (arcanum_formats::dialog::atoi(price.trim().as_bytes()), number_list(list))
        }
        None => (0, number_list(args)),
    }
}

/// The option line quest `n`'s row of [`GAME_QUEST`] gives for a quest `state` (0-6), PC
/// Intelligence and NPC reaction (0x4c4c00): tokens 16-22 below Intelligence 5, 9-15 below
/// reaction 20, else 2-8; -1 is none.
pub fn quest_option(row: &str, state: i32, intelligence: i32, reaction: i32) -> Option<i32> {
    let tokens: Vec<i32> = row.split_whitespace().map(|t| arcanum_formats::dialog::atoi(t.as_bytes())).collect();
    let set = if intelligence < 5 { 16 } else if reaction < 20 { 9 } else { 2 };
    tokens.get(set + state.clamp(0, 6) as usize).copied().filter(|&l| l != -1)
}

/// A walk to an NPC to talk to it; `seq` is the move it came with.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingTalk {
    pub npc: NetId,
    pub seq: u32,
    /// To pick its pocket, not to talk.
    pub steal: bool,
}

/// Requests taken from the inbox.
#[derive(Resource, Debug, Default)]
pub struct TalkRequests {
    pub talk: Vec<(PlayerId, NetId, u32)>,
    /// Pockets to pick ([`crate::pickpocket`]): the same walk, then the window.
    pub pockets: Vec<(PlayerId, NetId, u32)>,
    pub choose: Vec<(PlayerId, usize)>,
    pub end: Vec<PlayerId>,
    /// NPCs whose dialogue script ran the default: they bark at the PC.
    pub barks: Vec<(PlayerId, ScriptObject)>,
}

/// The reputation effects on reaction, read on first use.
#[derive(Resource, Debug, Default)]
pub struct ReputationTable(pub Option<crate::ReputationEffects>);

/// Tests and results the host met but cannot do yet: the dialogue file and the opcode.
#[derive(Resource, Debug, Default)]
pub struct DialogueReports(pub Vec<(u32, String)>);

/// The generic line files and NPC names, read on first use.
#[derive(Resource, Debug, Default)]
pub struct GenericLines(HashMap<String, Option<MesFile>>);

impl GenericLines {
    pub(crate) fn file(&mut self, vfs: Option<&arcanum_vfs::Vfs>, path: &str) -> Option<&MesFile> {
        self.0.entry(path.to_string()).or_insert_with(|| vfs.and_then(|v| v.read(path).ok()).and_then(|d| MesFile::parse(&d).ok())).as_ref()
    }
}

/// How a line floats (0x4d5fe0's colours).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FloatColour {
    /// An NPC's line.
    White,
    /// The PC's chosen line.
    Green,
    /// Combat text over a PC (`combat.mes` floats, table 0x5b8e6c).
    Red,
    /// Combat text over the player's followers.
    Yellow,
}

/// An NPC's reaction toward a PC (0x4c0d00): the PC's stored value for it (`npc_key`), else its
/// base, plus the modifier for the PC's looks, race and reputations.
pub fn reaction_to(pc: &PcScriptState, npc_key: &str, npc: &crate::NpcReaction, rules: &RulesData, sheet: &CharacterSheet, reputations: &crate::ReputationEffects) -> i32 {
    let stored = pc.reactions.get(npc_key).copied().unwrap_or(npc.base);
    let reputation = reputations.on(pc.reputations.iter().copied(), npc.origin, npc.faction);
    stored + npc.modifier(rules.derived(sheet).reaction, sheet.race, reputation)
}

/// What a step changes on the PC's sheet, applied after it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SheetChanges {
    /// Skills and the training rank they get.
    pub training: Vec<(usize, u8)>,
    /// The new base alignment.
    pub alignment: Option<i32>,
    pub experience: i32,
    pub fate_points: i32,
    /// Hit points healed and poison taken away.
    pub heal: i32,
    pub cure_poison: i32,
    /// A Master's Heal cures injuries ([`CharacterSheet::injuries`]).
    pub cure_injuries: bool,
}

impl SheetChanges {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Applies them to `sheet`. Experience is not: it is shared among the team
    /// ([`crate::share_experience`]).
    pub fn apply(&self, sheet: &mut CharacterSheet) {
        for &(skill, rank) in &self.training {
            sheet.training[skill] = rank;
        }
        if let Some(a) = self.alignment {
            sheet.alignment = a;
        }
        sheet.fate_points += self.fate_points;
        if !sheet.dead {
            sheet.hp_damage = (sheet.hp_damage - self.heal.max(0)).max(0);
        }
        sheet.poison = (sheet.poison - self.cure_poison.max(0)).max(0);
        if self.cure_injuries {
            sheet.injuries = 0;
        }
    }
}

/// `n` as a threshold: `n >= 0` at least `n`, `n < 0` at most `-n`.
pub fn passes(value: i32, n: i32) -> bool {
    if n < 0 { value <= -n } else { value >= n }
}

/// What tests and results read and change for one PC talking to one NPC.
pub struct Talk<'a> {
    /// Result `co`: the NPC attacks the PC; the talk ends ([`crate::combat`]).
    pub attack: bool,
    /// A PC of the party is dead: a healer offers Resurrect.
    pub party_dead: bool,
    /// Resurrect was bought: the dead PCs get up ([`crate::Resurrections`]).
    pub resurrect: bool,
    pub rules: &'a RulesData,
    pub sheet: &'a CharacterSheet,
    pub inventory: &'a mut Inventory,
    pub pc: &'a mut PcScriptState,
    pub globals: &'a mut ScriptGlobals,
    /// The NPC's dialogue attachment (flags and counters).
    pub npc: &'a mut Attachment,
    pub met: bool,
    /// The NPC's key in the PC's stored reactions, and what its reaction is made of.
    pub npc_key: String,
    pub npc_reaction: crate::NpcReaction,
    pub reputations: &'a crate::ReputationEffects,
    /// Gold changed: the inventory must be sent.
    pub gold_changed: bool,
    /// Opcodes not built yet that were met.
    pub unbuilt: Vec<String>,
    /// Changes to the PC's sheet this step: training, alignment, experience, fate points.
    pub sheet_changes: SheetChanges,
    /// The item prototypes (an item's `name` is what `in` and `ni` name).
    pub prototypes: &'a arcanum_formats::object::Prototypes,
    /// The NPC's `npc_flags` (tests `wa`, `wt`).
    pub npc_flags: i32,
    /// Following ([`crate::follow`]).
    pub follow: crate::follow::FollowTalk,
    /// The NPC's inventory, and its substitute container's (results `in -N`); changed ones are
    /// kept after the step.
    pub npc_inventory: Option<Inventory>,
    pub npc_stock: Option<Inventory>,
    pub npc_inventory_changed: bool,
    /// `Rules/xp_quest.mes`, for `xp`.
    pub xp_quest: Option<&'a MesFile>,
    /// Item facts, for moving items.
    pub item_info: &'a dyn Fn(u32) -> Option<crate::ItemInfo>,
    /// The area the PC is in ([`crate::areas`]; 0 none).
    pub pc_area: u32,
}

impl Talk<'_> {
    pub fn gold(&self) -> i32 {
        self.inventory.gold()
    }

    /// The PC's gold and its followers' here (0x415335: the `$$` test).
    pub fn party_gold(&self) -> i32 {
        self.gold() + self.follow.followers_gold.iter().map(|(_, g)| g).sum::<i32>()
    }

    /// `$$ n` (0x415cc6): gives `n`, or takes `-n` from the PC, then the rest from its followers
    /// in order.
    fn pay_from_party(&mut self, n: i32) {
        if n >= 0 {
            self.change_gold(n);
            return;
        }
        let from_pc = (-n).min(self.gold());
        self.change_gold(-from_pc);
        let mut rest = -n - from_pc;
        for (_, gold) in &mut self.follow.followers_gold {
            let take = rest.min(*gold);
            *gold -= take;
            rest -= take;
        }
    }

    /// An item's `name` (obj_f_name, field 0x16), what `in` / `ni` name.
    fn item_name(&self, prototype: u32) -> Option<i32> {
        self.prototypes.get(prototype)?.int("name")
    }

    /// Whether the PC carries an item named `name` (0x462480).
    fn has(&self, name: i32) -> bool {
        self.inventory.items.iter().any(|i| self.item_name(i.prototype) == Some(name))
    }

    /// Whether the NPC (or its substitute container) carries an item named `name`.
    fn npc_has(&self, name: i32) -> bool {
        [&self.npc_inventory, &self.npc_stock].into_iter().flatten().any(|inv| inv.items.iter().any(|i| self.item_name(i.prototype) == Some(name)))
    }

    /// The PC's training rank in `skill`, counting this step's changes.
    fn rank(&self, skill: usize) -> u8 {
        self.sheet_changes.training.iter().rev().find(|(k, _)| *k == skill).map_or(self.sheet.training[skill], |&(_, r)| r)
    }

    /// The base alignment (stat 0x13 as 0x4b0740 reads it: what `al` changes).
    fn alignment(&self) -> i32 {
        self.sheet_changes.alignment.unwrap_or(self.sheet.alignment)
    }

    /// The alignment with effects (0x4b0490: what the tests read).
    fn effective_alignment(&self) -> i32 {
        let effects = self.rules.alignment(self.sheet) - self.sheet.alignment;
        (self.alignment() + effects).clamp(-arcanum_rules::MAX_ALIGNMENT, arcanum_rules::MAX_ALIGNMENT)
    }

    /// What deciding whether the NPC joins this PC reads (0x4ad950).
    fn join_check(&self) -> crate::follow::JoinCheck {
        let f = &self.follow;
        crate::follow::JoinCheck {
            pc_persuasion_rank: self.sheet.training[crate::follow::PERSUASION],
            pc_charisma: self.rules.stats(self.sheet)[7],
            pc_alignment: self.effective_alignment(),
            pc_level: self.sheet.level,
            max_followers: self.rules.derived(self.sheet).max_followers,
            followers: f.followers,
            other_leader_charisma: f.other_leader_charisma,
            forced: self.npc_flags & crate::follow::FORCED != 0,
            reaction: self.reaction(),
            npc_alignment: f.npc_alignment,
            npc_level: f.npc_level,
            ai: f.ai,
        }
    }

    /// `jo force line` (0x41624d): the NPC joins this PC and the talk goes on at NPC line
    /// `line`, or it refuses. Forced, the count is not asked and the NPC is not counted.
    fn join(&mut self, force: bool, line: i32) {
        use crate::follow::{FORCED, LEFT, WAITING};
        match crate::follow::can_join(&self.join_check(), force) {
            Ok(()) => {
                let counted = !force && !(self.follow.mine && self.npc_flags & (WAITING | FORCED) == 0);
                self.follow.followers += i32::from(counted);
                self.follow.mine = true;
                self.follow.other_leader_charisma = None;
                self.npc_flags = (self.npc_flags & !(LEFT | WAITING)) | if force { FORCED } else { 0 };
                self.follow.changed = true;
                self.follow.goto = Some(line);
            }
            Err(r) => self.follow.refused = Some(r),
        }
    }

    /// Result `in` (0x415fed): `N > 0` the PC gives its first item named N to the NPC; `N < 0`
    /// the NPC (or its substitute container) gives one named -N to the PC. Nothing is made.
    /// A receiver without room: the original drops the item at its feet (the PC) or, for an
    /// NPC, drops it (a follower) or destroys it; here it stays where it was.
    fn give_item(&mut self, n: i32, info: &dyn Fn(u32) -> Option<crate::ItemInfo>, carry: i32, aptitude: i32) {
        let name = n.abs();
        if n > 0 {
            let Some(item) = self.inventory.items.iter().find(|i| self.item_name(i.prototype) == Some(name)).cloned() else { return };
            let Some(npc) = self.npc_inventory.as_mut() else { return };
            let mut changed = npc.clone();
            if changed.insert_item(crate::CarriedItem { location: 0, ..item.clone() }, info, i32::MAX, 0).is_ok() {
                *npc = changed;
                self.inventory.items.retain(|i| i.location != item.location);
                self.npc_inventory_changed = true;
                self.gold_changed = true;
            }
            return;
        }
        for source in [true, false] {
            let names = |inv: &Inventory| inv.items.iter().find(|i| self.prototypes.get(i.prototype).and_then(|p| p.int("name")) == Some(name)).cloned();
            let held = if source { self.npc_inventory.as_ref().and_then(names) } else { self.npc_stock.as_ref().and_then(names) };
            let Some(item) = held else { continue };
            let mut mine = self.inventory.clone();
            if mine.insert_item(crate::CarriedItem { location: 0, ..item.clone() }, info, carry, aptitude).is_err() {
                return;
            }
            *self.inventory = mine;
            let inv = if source { self.npc_inventory.as_mut() } else { self.npc_stock.as_mut() };
            if let Some(inv) = inv {
                inv.items.retain(|i| i.location != item.location);
            }
            self.npc_inventory_changed = true;
            self.gold_changed = true;
            return;
        }
    }

    /// Gives (`n > 0`) or takes (`n < 0`) gold; a PC with no gold yet gets a stack next to
    /// its first item (gold takes no grid room here; builder's call).
    fn change_gold(&mut self, n: i32) {
        if self.inventory.change_gold(n) {
            self.gold_changed = true;
        }
    }

    fn stat(&self, index: usize) -> i32 {
        self.rules.stats(self.sheet).get(index).copied().unwrap_or(0)
    }

    /// The NPC's reaction toward the PC (0x4c0d00): stored, else its base, plus the modifier.
    pub fn reaction(&self) -> i32 {
        reaction_to(self.pc, &self.npc_key, &self.npc_reaction, self.rules, self.sheet, self.reputations)
    }

    fn stored_reaction(&self) -> i32 {
        self.pc.reactions.get(&self.npc_key).copied().unwrap_or(self.npc_reaction.base)
    }


    /// Changes the reaction (dialogue `re`, 0x4c0de0): adds, or brings it to, at least or at most
    /// `n`; the change is stored for this PC.
    fn change_reaction(&mut self, n: i32, change: arcanum_formats::dialog::Change) {
        use arcanum_formats::dialog::Change;
        let now = self.reaction();
        let delta = match change {
            Change::Add => n,
            Change::Set => n - now,
            Change::AtLeast => (n - now).max(0),
            Change::AtMost => (n - now).min(0),
        };
        if delta != 0 {
            self.follow.reaction_dropped |= delta < 0 && self.follow.mine;
            let stored = self.stored_reaction() + delta;
            self.pc.reactions.insert(self.npc_key.clone(), stored);
        }
    }

    pub fn female(&self) -> bool {
        self.sheet.gender == FEMALE
    }

    /// The NPC has met the PC before (0x4c0c40: a stored reaction; here also a finished talk).
    pub fn has_met(&self) -> bool {
        self.met || self.pc.reactions.contains_key(&self.npc_key)
    }

    pub fn intelligence(&self) -> i32 {
        self.stat(4)
    }

    /// One test token (0x4150d0); `None` when the host cannot check it yet.
    fn test(&mut self, t: &Token) -> Option<bool> {
        let (a, b) = (t.a, t.b);
        Some(match t.op() {
            "gv" => self.globals.var(a) == b,
            "gf" => i32::from(self.globals.flag(a)) == b,
            "pv" => self.pc.vars.get(&(a as u32)).copied().unwrap_or(0) == b,
            "pf" => i32::from(self.pc.flags.contains(&(a as u32))) == b,
            "lf" => i32::from(self.npc.flags >> (a & 31) & 1 != 0) == b,
            "lc" => usize::try_from(a).ok().and_then(|i| self.npc.counters.get(i)).map_or(0, |&c| i32::from(c)) == b,
            "qu" => self.pc.quest(a) == b,
            "qa" => self.pc.quest(a) >= b,
            "qb" => self.pc.quest(a) <= b,
            "ss" => passes(self.globals.story_state, a),
            "ru" => self.pc.rumours.contains(&a.unsigned_abs()) == (a >= 0),
            "rq" => self.globals.quelled.contains(&a.unsigned_abs()) == (a >= 0),
            "rp" => self.pc.reputations.contains(&a.unsigned_abs()) == (a > 0),
            "me" => self.has_met() == (a != 0),
            "re" => passes(self.reaction(), a),
            "$$" => passes(self.party_gold(), a),
            "in" if a > 0 => self.has(a),
            "in" => self.npc_has(-a),
            "ni" if a > 0 => !self.has(a),
            "ni" => !self.npc_has(-a),
            // Alignment (stat 0x13), the aptitude (stat 0x10: magick up, tech down), and "na"
            // (0x4159bc: alignment at least -a, or at most a when a is negative).
            "al" => passes(self.effective_alignment(), a),
            "ma" => passes(self.rules.aptitude(self.sheet), a),
            "ta" => passes(-self.rules.aptitude(self.sheet), a),
            "na" => if a >= 0 { self.effective_alignment() >= -a } else { self.effective_alignment() <= a },
            // Training rank and skill level of skill S (12-15 the tech skills), spell college C.
            "tr" => usize::try_from(a).ok().filter(|&k| k < arcanum_rules::SKILLS).is_some_and(|k| passes(i32::from(self.rank(k)), b)),
            "sk" => usize::try_from(a).ok().filter(|&k| k < arcanum_rules::SKILLS).is_some_and(|k| passes(self.rules.skill_level(self.sheet, k), b)),
            "sc" => usize::try_from(a).ok().and_then(|c| self.sheet.spells.get(c)).is_some_and(|&l| if b > 0 { l >= b } else { l <= -b }),
            // Follows the PC (nobody follows anyone yet): 0 not, 1 yes, others always pass.
            "fo" => self.follow.mine == (a != 0),
            // A party member named N: none yet.
            "pa" => a < 0,
            // The NPC's `npc_flags` 0x8 / 0x20: 0 clear, else set.
            "wa" => (self.npc_flags & 0x8 != 0) == (a != 0),
            "wt" => (self.npc_flags & 0x20 != 0) == (a != 0),
            "ch" => passes(self.stat(7), a),
            "pe" => passes(self.stat(5), a),
            "le" => passes(self.sheet.level, a),
            "ra" => (i32::from(self.sheet.race) + 1 == a.abs()) == (a > 0),
            "ps" => passes(self.rules.skill_level(self.sheet, 11), a),
            "ha" => passes(self.rules.skill_level(self.sheet, 9), a),
            // An area known; the PC in area N (`-N`: not).
            "ar" => self.pc.areas.contains(&a.unsigned_abs()) == (a > 0),
            "ia" => (self.pc_area == a.unsigned_abs()) == (a > 0),
            _ => return None,
        })
    }

    /// Whether a test passes: every token must (an unknown opcode fails it).
    pub fn passes_test(&mut self, text: &str) -> bool {
        let Ok(tokens) = tests(text) else { return false };
        tokens.iter().all(|t| match self.test(t) {
            Some(passed) => passed,
            None => {
                self.unbuilt.push(format!("test {}", t.op()));
                false
            }
        })
    }

    /// Runs a result (0x415ba0) left to right; an unknown opcode stops it. Returns the line `fl`
    /// closes with, if any (it stops the result too).
    pub fn run_result(&mut self, text: &str) -> Option<i32> {
        let tokens = match effects(text) {
            Ok(t) => t,
            Err(e) => e.before,
        };
        for t in &tokens {
            let (a, b) = (t.a, t.b);
            match t.op() {
                "$$" => self.pay_from_party(a),
                "gv" => self.globals.set_var(a, b),
                "gf" => self.globals.set_flag(a, b != 0),
                "pv" => set_or_remove(&mut self.pc.vars, a as u32, b),
                "pf" => set_in(&mut self.pc.flags, a as u32, b != 0),
                "lf" => {
                    let bit = 1 << (a & 31);
                    self.npc.flags = if b != 0 { self.npc.flags | bit } else { self.npc.flags & !bit };
                }
                "lc" => {
                    if let Some(c) = usize::try_from(a).ok().and_then(|i| self.npc.counters.get_mut(i)) {
                        *c = b as u8;
                    }
                }
                "qu" => set_or_remove(&mut self.pc.quests, a as u32, b),
                "ss" => self.globals.story_state = self.globals.story_state.max(a),
                "ru" => set_in(&mut self.pc.rumours, a.unsigned_abs(), true),
                "rq" => set_in(&mut self.globals.quelled, a.unsigned_abs(), true),
                "rp" => set_in(&mut self.pc.reputations, a.unsigned_abs(), a > 0),
                "fl" => return Some(a),
                "mm" => set_in(&mut self.pc.areas, a.unsigned_abs(), true),
                "in" => {
                    let info = self.item_info;
                    let (carry, aptitude) = (self.rules.derived(self.sheet).carry_weight, self.rules.aptitude(self.sheet));
                    self.give_item(a, info, carry, aptitude);
                }
                // Alignment (0x415f19): set, add (+/-), at least (>), at most (<), as `re`.
                "al" => {
                    use arcanum_formats::dialog::Change;
                    let now = self.alignment();
                    self.sheet_changes.alignment = Some(match t.change {
                        Change::Add => now + a,
                        Change::Set => a,
                        Change::AtLeast => now.max(a),
                        Change::AtMost => now.min(a),
                    });
                }
                // Training (0x4161eb): only one rank up (or down) at a time, with the level the rank needs.
                "tr" => {
                    if let (Ok(skill), Ok(rank)) = (usize::try_from(a), u8::try_from(b))
                        && skill < arcanum_rules::SKILLS
                        && rank <= 3
                    {
                        let now = self.rank(skill);
                        let step_ok = !(now < rank && now + 1 != rank);
                        if step_ok && self.rules.skill_level(self.sheet, skill) >= RANK_LEVELS[rank as usize] {
                            self.sheet_changes.training.push((skill, rank));
                        }
                    }
                }
                // Experience from `Rules/xp_quest.mes` line N (0x4c53c0 → 0x45f110), normal difficulty.
                "xp" => {
                    let amount = self.xp_quest.and_then(|m| m.get(a.max(0) as u32)).map_or(0, |t| arcanum_formats::dialog::atoi(t.as_bytes()));
                    self.sheet_changes.experience += amount;
                }
                "fp" => self.sheet_changes.fate_points += 1,
                "jo" => self.join(a != 0, b),
                "uw" => {
                    if self.follow.mine && self.npc_flags & crate::follow::WAITING != 0 {
                        match crate::follow::can_join(&self.join_check(), a != 0) {
                            Ok(()) => {
                                self.npc_flags &= !crate::follow::WAITING;
                                self.follow.waits = None;
                                self.follow.followers += i32::from(self.npc_flags & crate::follow::FORCED == 0);
                                self.follow.changed = true;
                                self.follow.goto = Some(b);
                            }
                            Err(r) => self.follow.refused = Some(r),
                        }
                    }
                }
                "lv" => {
                    if self.follow.mine || self.follow.other_leader_charisma.is_some() {
                        self.follow.mine = false;
                        self.follow.other_leader_charisma = None;
                        self.npc_flags &= !(crate::follow::FORCED | crate::follow::WAITING);
                        self.follow.changed = true;
                    }
                }
                "wa" => {
                    if self.follow.mine && self.npc_flags & crate::follow::WAITING == 0 {
                        self.follow.followers -= i32::from(self.npc_flags & crate::follow::FORCED == 0);
                        self.npc_flags |= crate::follow::WAITING;
                        self.follow.changed = true;
                        let rank = self.sheet.training[crate::follow::PERSUASION];
                        self.follow.waits = Some(crate::follow::wait_delay_ms(self.follow.npc_wait, rank, self.stat(7)));
                    }
                }
                "sc" | "so" => {
                    let flags = if t.op() == "so" { self.npc_flags | crate::follow::SPREAD_OUT } else { self.npc_flags & !crate::follow::SPREAD_OUT };
                    self.follow.changed |= flags != self.npc_flags;
                    self.npc_flags = flags;
                }
                "re" => self.change_reaction(a, t.change),
                // Combat (0x415ba0 `co`): the NPC fights the PC; the talk is over.
                "co" => {
                    self.attack = true;
                    return Some(0);
                }
                op => self.unbuilt.push(format!("result {op}")),
            }
        }
        None
    }
}

fn set_or_remove(map: &mut std::collections::BTreeMap<u32, i32>, key: u32, value: i32) {
    if value == 0 {
        map.remove(&key);
    } else {
        map.insert(key, value);
    }
}

fn set_in(set: &mut BTreeSet<u32>, key: u32, on: bool) {
    if on {
        set.insert(key);
    } else {
        set.remove(&key);
    }
}

/// A PC option's generic range (0x416c10), for the prefixes built.
pub fn generic_range(text: &str) -> Option<std::ops::Range<u32>> {
    Some(match text.trim() {
        "Y:" | "y:" => 1..100,
        "N:" | "n:" => 100..200,
        "S:" | "s:" => 200..300,
        "E:" | "e:" => 400..500,
        "F:" | "f:" => 800..900,
        "K:" | "k:" => 1500..1600,
        "W:" | "w:" => 1800..1900,
        _ => return None,
    })
}

/// Whether an option's text is a generic prefix (`X:` or `X:args`).
pub fn is_prefixed(text: &str) -> bool {
    let b = text.trim_start().as_bytes();
    b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':'
}

/// The prefix letter of a generic text, upper case.
fn prefix(text: &str) -> Option<char> {
    is_prefixed(text).then(|| text.trim_start().as_bytes()[0].to_ascii_uppercase() as char)
}

/// What a `G:` argument says for its case (0x419260).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GreetingArg {
    /// `k n`: go to NPC line `n`, with its options.
    Line(i32),
    /// `k fl n`: close with line `n`.
    Closing(i32),
    /// `k 0`: the case is off (for `k` 9: say nothing).
    Off,
}

/// The arguments after `G:`: `k line` or `k fl line`, comma separated, `k` 0-9.
pub fn greeting_args(text: &str) -> [Option<GreetingArg>; 10] {
    use arcanum_formats::dialog::atoi;
    let mut out = [None; 10];
    for part in text.split(',') {
        let mut words = part.split_whitespace();
        let Some(k) = words.next().and_then(|w| usize::try_from(atoi(w.as_bytes())).ok()).filter(|&k| k < 10) else { continue };
        out[k] = match words.next() {
            Some(w) if w.eq_ignore_ascii_case("fl") => Some(GreetingArg::Closing(words.next().map_or(0, |n| atoi(n.as_bytes())))),
            Some(w) => Some(match atoi(w.as_bytes()) {
                0 => GreetingArg::Off,
                n => GreetingArg::Line(n),
            }),
            None => continue,
        };
    }
    out
}

/// The greeting block for a reaction level (0x419260's tables 0x4197a8 and 0x41978c): class
/// (`true`) or race, and its base. Levels past 6 have none.
pub fn greeting_block(met: bool, level: u32) -> Option<(bool, u32)> {
    Some(match (met, level) {
        (false, 0) => (true, 8000),
        (false, 1 | 2) => (true, 9000),
        (false, 3) => (true, 10000),
        (false, 4 | 5) => (false, 2000),
        (false, 6) => (false, 3000),
        (true, 0) => (true, 11000),
        (true, 1 | 2) => (true, 12000),
        (true, 3) => (true, 13000),
        (true, 4 | 5) => (false, 4000),
        (true, 6) => (false, 5000),
        _ => return None,
    })
}

/// The file name part for a speaker of `from_male` to a listener (`m2f`: male to female).
fn genders(from_male: bool, to_male: bool) -> &'static str {
    match (from_male, to_male) {
        (true, true) => "m2m",
        (true, false) => "m2f",
        (false, true) => "f2m",
        (false, false) => "f2f",
    }
}

/// The options after NPC line `line` that `talk` may choose (0x414f50): the PC options up to the
/// next NPC line, by gender, intelligence and test, at most [`MAX_OPTIONS`].
pub fn options(file: &DialogFile, line: i32, talk: &mut Talk) -> Vec<(i32, DialogLine)> {
    let mut out = Vec::new();
    let iq = talk.intelligence();
    for (&n, entry) in file.lines.range(line + 1..) {
        if entry.is_npc() || out.len() == MAX_OPTIONS {
            break;
        }
        if entry.for_males().is_some_and(|males| males == talk.female()) {
            continue;
        }
        if !passes(iq, entry.intelligence()) {
            continue;
        }
        if !entry.test.trim().is_empty() && !talk.passes_test(&entry.test) {
            continue;
        }
        out.push((n, entry.clone()));
    }
    out
}

/// Puts the names in (0x416b00): `@pcname@` and `@npcname@`, any case.
pub fn with_names(text: &str, pc: &str, npc: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('@') {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        let lower = tail.to_ascii_lowercase();
        if lower.starts_with("@pcname@") {
            out.push_str(pc);
            rest = &tail[8..];
        } else if lower.starts_with("@npcname@") {
            out.push_str(npc);
            rest = &tail[9..];
        } else {
            out.push('@');
            rest = &tail[1..];
        }
    }
    out.push_str(rest);
    out
}

/// The voice file of line `voice` of dialogue `dialog` for a PC of that gender (0x5688d0;
/// builder's call: the PC's gender, as the line's text). The client falls back to `m`.
pub fn voice_path(dialog: u32, voice: i32, female: bool) -> Option<String> {
    (voice > 0 && dialog > 0).then(|| format!("sound/speech/{:05}/v{}_{}.mp3", dialog & 0x7fff, voice, if female { 'f' } else { 'm' }))
}

type WalkerQuery<'a> = (Entity, &'a PlayerControlled, &'a Position, Option<&'a PendingTalk>, Has<crate::Walking>, Has<Conversation>);
type TalkerQuery<'a> = (Entity, &'a NetId, &'a PlayerControlled, &'a Position, &'a mut Sheet, &'a mut Inventory, Option<&'a mut Conversation>, Option<&'a PendingTalk>, Has<crate::Walking>);

/// Walks PCs to the NPCs they want to talk to and runs those NPCs' dialogue scripts.
#[allow(clippy::too_many_arguments)]
pub fn talk_to_npcs(
    mut commands: Commands,
    mut requests: ResMut<TalkRequests>,
    mut world: ResMut<WorldState>,
    mut map: ResMut<LoadedMap>,
    mut outbox: ResMut<Outbox>,
    mut scripts: crate::ScriptHost,
    mut pockets: ResMut<crate::pickpocket::PickPocketStarts>,
    pcs: Query<WalkerQuery>,
    npcs: Query<(&NetId, &NpcSource, &Position)>,
) {
    let mut fresh: HashMap<Entity, PendingTalk> = HashMap::new();
    let talks = requests.talk.drain(..).map(|r| (r, false)).collect::<Vec<_>>();
    for ((player, npc, seq), steal) in talks.into_iter().chain(requests.pockets.drain(..).map(|r| (r, true))) {
        if let Some((entity, ..)) = pcs.iter().find(|pc| pc.1.0 == player) {
            fresh.insert(entity, PendingTalk { npc, seq, steal });
        }
    }
    for (entity, player, position, pending, walking, talking) in &pcs {
        let Some(want) = fresh.get(&entity).copied().or(pending.copied()) else { continue };
        let Some((_, source, at)) = npcs.iter().find(|n| *n.0 == want.npc) else {
            commands.entity(entity).remove::<PendingTalk>();
            continue;
        };
        if !crate::within_radius(position.0, at.0, crate::USE_RANGE) {
            if walking {
                commands.entity(entity).insert(want);
            } else {
                commands.entity(entity).remove::<PendingTalk>();
            }
            continue;
        }
        commands.entity(entity).remove::<PendingTalk>();
        if want.steal {
            let can = !map.is_dead(source.sector, source.index) && map.mobiles.get(&source.sector).and_then(|m| m.get(source.index as usize)).is_some_and(|o| crate::pickpocket::can_steal_from(o, &map.prototypes));
            if can && !talking {
                pockets.0.push((player.0, want.npc, source.object()));
            } else if !talking {
                outbox.0.push((player.0, LedgerDelta::SkillResult { message: crate::pickpocket::CANNOT_STEAL }));
            }
            continue;
        }
        // The dead do not talk (builder's call: the original's can-talk test 0x4ad800 is not
        // read).
        if talking || map.is_dead(source.sector, source.index) {
            continue;
        }
        // The NPC's dialogue script (0x57cc70); its action 4 starts the conversation. If it runs
        // the default, the NPC barks (0x5681c0).
        if scripts.hook(&mut world, &mut map, &mut outbox, source.object(), EVENT_DIALOGUE, player.0) == crate::HookResult::RunDefault {
            requests.barks.push((player.0, source.object()));
        }
    }
}

/// What a conversation step writes to besides the world: the outbox, barters, areas, the
/// experience earned, and the game clock it reads.
type ConversationOut<'w> = (ResMut<'w, Outbox>, ResMut<'w, crate::barter::BarterStarts>, ResMut<'w, crate::areas::AreaTable>, ResMut<'w, crate::ExperienceAwards>, Res<'w, crate::GameClock>, ResMut<'w, crate::combat::CombatStarts>, ResMut<'w, crate::Resurrections>);

/// Starts, steps and ends conversations: see the module docs.
#[allow(clippy::too_many_arguments)]
pub fn run_conversations(
    mut commands: Commands,
    mut started: ResMut<crate::StartedDialogues>,
    mut requests: ResMut<TalkRequests>,
    mut world: ResMut<WorldState>,
    mut map: ResMut<LoadedMap>,
    mut scripts: ResMut<crate::Scripts>,
    mut generic: ResMut<GenericLines>,
    mut reports: ResMut<DialogueReports>,
    (session, rules, vfs, acks): (Res<Session>, Res<SimRules>, Option<Res<crate::SimVfs>>, Res<crate::InventoryAcks>),
    (mut rng, mut reputations): (ResMut<crate::HostRng>, ResMut<ReputationTable>),
    (mut outbox, mut barters, mut areas, mut awards, clock, mut fights, mut resurrections): ConversationOut,
    mut pcs: Query<TalkerQuery>,
    npcs: Query<(&NetId, &NpcSource)>,
) {
    if started.0.is_empty() && requests.choose.is_empty() && requests.end.is_empty() && requests.barks.is_empty() {
        return;
    }
    let vfs_guard = vfs.as_ref().and_then(|v| v.0.read().ok());
    let vfs = vfs_guard.as_deref();
    let library = scripts.0.get_or_insert_with(|| vfs.map(crate::ScriptLibrary::new).unwrap_or_default());
    areas.load(vfs);
    let reputations = &*reputations.0.get_or_insert_with(|| {
        vfs.and_then(|v| v.read(crate::GAME_REP).ok()).and_then(|d| MesFile::parse(&d).ok()).map(|m| crate::ReputationEffects::parse(&m)).unwrap_or_default()
    });
    let mut steps: Vec<(PlayerId, Step)> = started.0.drain(..).map(|s| (s.player, Step::Start(s))).collect();
    steps.extend(requests.choose.drain(..).map(|(p, i)| (p, Step::Choose(i))));
    steps.extend(requests.end.drain(..).map(|p| (p, Step::End)));
    steps.extend(requests.barks.drain(..).map(|(p, npc)| (p, Step::Bark(npc))));
    // Every PC's charisma, for an NPC that already follows another (0x4ad950).
    let charisma: BTreeMap<PlayerId, i32> = pcs.iter().map(|pc| (pc.2.0, rules.0.stats(&pc.4.0)[7])).collect();
    for (player, step) in steps {
        let Some((entity, &pc_net, _, &Position(pc_at), mut sheet, mut inventory, conversation, ..)) = pcs.iter_mut().find(|pc| pc.2.0 == player) else { continue };
        let Some(save) = world.0.as_mut() else { continue };
        let party_dead = save.players.values().any(|p| p.blueprint.sheet.dead);
        let Some(saved) = save.players.get_mut(&player) else { continue };
        let (npc_object, dialog, current) = match (&step, conversation.as_deref()) {
            (Step::Start(s), _) => (s.npc, s.dialog, None),
            (Step::Bark(_), Some(_)) => continue,
            (Step::Bark(npc), None) => (*npc, 0, None),
            (_, Some(c)) => (c.npc_object, c.dialog, Some(c.clone())),
            (_, None) => continue,
        };
        let npc_net = match npc_object {
            ScriptObject::Mobile { sector, index } => npcs.iter().find(|n| n.1.sector == sector && n.1.index == index).map(|n| *n.0),
            _ => None,
        };
        let Some(npc_net) = npc_net else { continue };
        let file = match step {
            Step::Bark(_) => Default::default(),
            _ => match vfs.and_then(|v| library.dialog(v, dialog)) {
                Some(file) => file,
                None => continue,
            },
        };
        let key = met_key(npc_object);
        let mut npc_attachment = map.attachment(npc_object, EVENT_DIALOGUE);
        let before = npc_attachment;
        let mut npc_info = npc_details(&map, npc_object, &mut generic, vfs);
        let override_number = map.attachment(npc_object, EVENT_OVERRIDE).number;
        if override_number != 0 {
            npc_info.overrides = vfs.and_then(|v| library.dialog(v, override_number)).map(|f| (override_number, f));
        }
        let met = saved.script_state.met.contains(&key);
        let prototypes = map.prototypes.clone();
        let art = map.art.clone();
        let item_info = crate::item_info(&prototypes, |id| crate::world::art_size(vfs, art.as_deref(), id));
        let (npc_flags, npc_inv, stock_id) = match npc_object {
            ScriptObject::Mobile { sector, index } => (
                map.mobiles.get(&sector).and_then(|m| m.get(index as usize)).and_then(|o| map.prototypes.int(o, "npc_flags")).unwrap_or(0),
                map.npc_inventory(sector, index),
                map.substitute(sector, index),
            ),
            _ => (0, None, None),
        };
        let npc_stock = stock_id.and_then(|id| map.pile(id)).map(|p| p.inventory);
        let xp_quest = generic.file(vfs, XP_QUEST).cloned();
        let areas = &*areas;
        let pc_area = map.name.as_deref().map_or(0, |m| areas.area_at(m, pc_at));
        let follow = match npc_object {
            ScriptObject::Mobile { sector, index } => {
                let home = map.home_of(sector, index);
                let record = save.followers.iter().find(|f| f.is(&home));
                let counted = save
                    .followers
                    .iter()
                    .filter(|f| f.leader == player && crate::follow::follower_flags(f, &map, &save.other_maps) & (crate::follow::WAITING | crate::follow::FORCED) == 0)
                    .count();
                let object = map.mobiles.get(&sector).and_then(|m| m.get(index as usize));
                let stat = |i: usize| object.and_then(|o| o.ints("critter_stat_base_idx").or_else(|| map.prototypes.of(o)?.ints("critter_stat_base_idx"))).and_then(|v| v.get(i).copied()).unwrap_or(0);
                let packet = object.and_then(|o| map.prototypes.int(o, "npc_ai_data")).unwrap_or(0);
                // The followers' gold: those here, not waiting, of this PC.
                let followers_gold = save
                    .followers
                    .iter()
                    .filter(|f| f.leader == player && crate::follow::follower_flags(f, &map, &save.other_maps) & crate::follow::WAITING == 0)
                    .filter_map(|f| map.place_of(&f.home()))
                    .map(|(s, i)| ((s, i), map.npc_inventory(s, i).map_or(0, |inv| inv.gold())))
                    .collect();
                crate::follow::FollowTalk {
                    followers_gold,
                    npc_wait: object.and_then(|o| map.prototypes.int(o, "npc_wait")).unwrap_or(crate::follow::DEFAULT_NPC_WAIT),
                    mine: record.is_some_and(|f| f.leader == player),
                    other_leader_charisma: record.filter(|f| f.leader != player).map(|f| charisma.get(&f.leader).copied().unwrap_or(0)),
                    followers: counted as i32,
                    ai: crate::follow::ai_follow(generic.file(vfs, crate::follow::GAME_AI), packet),
                    npc_alignment: stat(0x13),
                    npc_level: stat(0x11),
                    ..Default::default()
                }
            }
            _ => Default::default(),
        };
        let mut talk = Talk {
            attack: false,
            party_dead,
            resurrect: false,
            follow,
            rules: &rules.0,
            sheet: &sheet.0,
            inventory: &mut inventory,
            pc: &mut saved.script_state,
            globals: &mut save.scripts,
            npc: &mut npc_attachment,
            met,
            npc_key: key.clone(),
            npc_reaction: npc_info.reaction,
            reputations,
            gold_changed: false,
            unbuilt: Vec::new(),
            sheet_changes: SheetChanges::default(),
            pc_area,
            prototypes: &prototypes,
            npc_flags,
            npc_inventory: npc_inv.clone(),
            npc_stock: npc_stock.clone(),
            npc_inventory_changed: false,
            xp_quest: xp_quest.as_ref(),
            item_info: &item_info,
        };
        // "Force good reaction" (fate option 1, 0x4c1020): a talk starting below 100 starts at
        // 100, and the fate use is spent.
        let forced = matches!(step, Step::Start(_)) && sheet.0.fate_pending & crate::fate_bit(crate::FATE_GOOD_REACTION) != 0 && talk.reaction() < 100;
        if forced {
            talk.change_reaction(100, arcanum_formats::dialog::Change::Set);
        }
        let mut say = Say { npc_net, dialog, pc_name: &sheet.0.name, npc: &npc_info, generic: &mut generic, vfs, rng: &mut rng, deltas: Vec::new(), jumps: 0, areas, pc_at };
        let next = match step {
            Step::Start(s) => say.show(&file, s.line, &mut talk),
            Step::Choose(i) => match current.as_ref().and_then(|c| c.options.get(i)) {
                Some(offered) => {
                    say.float(pc_net, offered.text.clone(), FloatColour::Green, None);
                    match offered.act {
                        OptionAct::Line(line) => say.chosen(&file, line, &mut talk, 0),
                        OptionAct::Respond(n) => say.respond(&file, n, &mut talk, 0),
                        OptionAct::Train { skill, ret } => say.train(&file, skill, ret, &mut talk),
                        OptionAct::Pay { price, service, ret } => say.pay(&file, price, service, ret, &mut talk),
                        OptionAct::Direct { area, price, mark, ret } => {
                            if price > 0 {
                                say.ask_price(&file, price, Service::Directions { area, mark }, ret, &mut talk)
                            } else {
                                say.direct(&file, area, mark, ret, &mut talk)
                            }
                        }
                        OptionAct::MoreDirections { option, offset, ret } => say.directions(&file, option, offset, ret, &mut talk),
                        OptionAct::HealPage { page, ret } => say.heal_menu(&file, page, ret, &mut talk),
                        OptionAct::Heal { healing, ret } => say.heal(&file, healing, ret, &mut talk),
                        OptionAct::Headline { ret } => say.headline(&file, ret, &mut talk),
                        OptionAct::OldPapers { ret } => say.old_papers(&file, ret, &mut talk),
                        OptionAct::BuyPaper { paper, ret } => say.ask_price(&file, NEWSPAPER_PRICE, Service::Newspaper(paper), ret, &mut talk),
                    }
                }
                None => Shown::Stay,
            },
            Step::End => Shown::End,
            Step::Bark(_) => {
                say.greeting("", &mut talk);
                Shown::Stay
            }
        };
        let attack = std::mem::take(&mut talk.attack);
        if std::mem::take(&mut talk.resurrect) {
            resurrections.0 += 1;
        }
        let next = if attack { Shown::End } else { next };
        let gold_changed = talk.gold_changed;
        let follow_change = talk.follow.changed.then_some((talk.follow.mine, talk.npc_flags));
        let follow_waits = talk.follow.waits;
        let reaction_dropped = talk.follow.reaction_dropped;
        let followers_gold = std::mem::take(&mut talk.follow.followers_gold);
        let sheet_changes = std::mem::take(&mut talk.sheet_changes);
        let npc_changes = talk.npc_inventory_changed.then(|| (talk.npc_inventory.take(), talk.npc_stock.take()));
        for op in talk.unbuilt.drain(..) {
            reports.0.push((dialog, op));
        }
        let excess = reports.0.len().saturating_sub(crate::REPORTS_KEPT);
        reports.0.drain(..excess);
        let deltas = std::mem::take(&mut say.deltas);
        let mut ended = false;
        match next {
            Shown::Line(line, options) => {
                let texts = options.iter().map(|o| o.text.clone()).collect();
                commands.entity(entity).insert(Conversation { npc: npc_net, npc_object, dialog, line, options });
                outbox.0.push((player, LedgerDelta::DialogueOptions { npc: npc_net, options: texts }));
            }
            Shown::End => ended = true,
            Shown::Barter => {
                ended = true;
                barters.0.push((player, npc_net, npc_object));
            }
            Shown::Stay => {}
        }
        if attack {
            fights.0.push((npc_net, pc_net));
        }
        if ended {
            commands.entity(entity).remove::<Conversation>();
            saved.script_state.met.insert(key);
            outbox.0.push((player, LedgerDelta::DialogueEnded));
        }
        let mut touched = Vec::new();
        // A join, leave, wait or order: the follower list and the NPC's flags (a guest's on its
        // record, else in this map's changes). A guest that leaves goes home at the next load.
        if let (Some((mine, flags)), ScriptObject::Mobile { sector, index }) = (follow_change, npc_object) {
            touched.extend(crate::follow::set_follower(&mut save.followers, &mut map, (sector, index), mine.then_some(player), flags, follow_waits, clock.now));
        }
        // A follower's reaction dropped: it warns at its next check (0x4c0de0 sets 0x10000).
        if reaction_dropped && let ScriptObject::Mobile { sector, index } = npc_object {
            let home = map.home_of(sector, index);
            if let Some(f) = save.followers.iter_mut().find(|f| f.is(&home)) {
                f.warn |= crate::follow::WARN_REACTION;
            }
        }
        // The followers' gold paid out.
        for ((sector, index), gold) in followers_gold {
            if let Some(mut inventory) = map.npc_inventory(sector, index)
                && inventory.gold() != gold
            {
                inventory.change_gold(gold - inventory.gold());
                map.set_npc_inventory(sector, index, inventory);
                touched.push(sector);
            }
        }
        if let (Some((npc_inv, npc_stock)), ScriptObject::Mobile { sector, index }) = (npc_changes, npc_object) {
            if let Some(inv) = npc_inv {
                map.set_npc_inventory(sector, index, inv);
            }
            if let (Some(stock), Some(id)) = (npc_stock, stock_id)
                && let Some(mut pile) = map.pile(id)
            {
                pile.inventory = stock;
                map.set_pile(id, Some(pile));
                touched.push(id.sector());
            }
            touched.push(sector);
        }
        if sheet_changes.experience != 0 {
            awards.0.push(sheet_changes.experience);
        }
        if forced && crate::consume_fate(&mut sheet.0, crate::FATE_GOOD_REACTION) {
            saved.blueprint.sheet = sheet.0.clone();
            outbox.0.push((player, LedgerDelta::SheetUpdate { ack: 0, sheet: sheet.0.clone() }));
        }
        // The PC's alignment moved: every follower warns at its next check (0x4b0980 stat 0x13).
        if let Some(new) = sheet_changes.alignment
            && new != sheet.0.alignment
        {
            let bit = if new >= sheet.0.alignment { crate::follow::WARN_GOOD } else { crate::follow::WARN_EVIL };
            for f in save.followers.iter_mut().filter(|f| f.leader == player) {
                f.warn |= bit;
            }
        }
        if !sheet_changes.is_empty() && sheet_changes != (SheetChanges { experience: sheet_changes.experience, ..Default::default() }) {
            sheet_changes.apply(&mut sheet.0);
            saved.blueprint.sheet = sheet.0.clone();
            outbox.0.push((player, LedgerDelta::SheetUpdate { ack: 0, sheet: sheet.0.clone() }));
            crate::ground::broadcast(&session, &mut outbox, LedgerDelta::PcStatus { net_id: pc_net, status: crate::pc_status(&rules.0, &sheet.0) });
        }
        if gold_changed {
            saved.inventory = Some(inventory.clone());
            let ack = acks.0.get(&player).copied().unwrap_or(0);
            outbox.0.push((player, LedgerDelta::Inventory { net_id: pc_net, ack, inventory: inventory.clone() }));
        }
        for delta in deltas {
            crate::ground::broadcast(&session, &mut outbox, delta);
        }
        if npc_attachment != before {
            map.set_attachment(npc_object, EVENT_DIALOGUE, npc_attachment);
            if let ScriptObject::Mobile { sector, .. } = npc_object {
                touched.push(sector);
            }
        }
        for sector in touched {
            crate::ground::save_change(&mut world, &map, sector);
        }
    }
}

enum Step {
    Start(StartedDialogue),
    Choose(usize),
    End,
    Bark(ScriptObject),
}

/// What follows a step.
pub enum Shown {
    /// An NPC line with these options.
    Line(i32, Vec<Offered>),
    End,
    /// Nothing changes (a choice that is not there, a bark).
    Stay,
    /// The talk ends and barter opens (state 3, `0x572240(pc, npc, 1)`).
    Barter,
}

/// What the generic lines need of the NPC.
#[derive(Debug, Clone)]
pub struct NpcInfo {
    pub name: String,
    pub male: bool,
    pub reaction: crate::NpcReaction,
    /// `npc_social_class` (0x45d010): the class block of `gd_cls` files.
    pub class: i32,
    /// Intelligence 4 or less (0x45fc00): the `dumb` files.
    pub dumb: bool,
    /// The override dialogue (its number and file).
    pub overrides: Option<(u32, std::sync::Arc<DialogFile>)>,
    /// What it heals with.
    pub healer: Healer,
}

/// NPCs each player has been told it knows, by net id.
#[derive(Resource, Debug, Default)]
pub struct KnownSent(pub BTreeMap<PlayerId, std::collections::BTreeSet<NetId>>);

/// Tells each player which NPCs in the world its PC has met (talked to, or holding a reaction),
/// once each ([`LedgerDelta::KnownNpcs`]): the client shows their names (0x45f970: met NPCs
/// by `description`, others by `critter_description_unknown`). Every 15 ticks.
pub fn send_known_npcs(world: Res<crate::WorldState>, tick: Res<crate::SimTick>, npcs: Query<(&NetId, &crate::NpcSource)>, mut sent: ResMut<KnownSent>, mut outbox: ResMut<crate::Outbox>) {
    if !tick.0.is_multiple_of(15) {
        return;
    }
    let Some(save) = world.0.as_ref() else { return };
    for (&player, saved) in &save.players {
        let state = &saved.script_state;
        let told = sent.0.entry(player).or_default();
        let new: Vec<NetId> = npcs
            .iter()
            .filter(|(id, source)| !told.contains(id) && {
                let key = met_key(source.object());
                state.met.contains(&key) || state.reactions.contains_key(&key)
            })
            .map(|(&id, _)| id)
            .collect();
        if !new.is_empty() {
            told.extend(new.iter().copied());
            outbox.0.push((player, LedgerDelta::KnownNpcs { npcs: new }));
        }
    }
}

/// The NPC's key in a PC's stored reactions and NPCs met.
pub fn met_key(npc: ScriptObject) -> String {
    match npc {
        ScriptObject::Mobile { sector, index } => format!("{}:{}", sector.0, index),
        ScriptObject::Map(id) => format!("map {}:{}", id.sector.0, id.index),
        ScriptObject::Pc(p) => format!("pc {}", p.0),
        ScriptObject::Carried { holder, prototype } => format!("item {} of {}", prototype, holder.0),
        ScriptObject::NpcCarried { sector, index, prototype } => format!("item {prototype} of {sector:?}/{index}"),
    }
}

/// The NPC's name (`mes/description.mes` by its `description`) and gender (stat 26, 1 male).
pub fn npc_details(map: &LoadedMap, npc: ScriptObject, generic: &mut GenericLines, vfs: Option<&arcanum_vfs::Vfs>) -> NpcInfo {
    let object = match npc {
        ScriptObject::Mobile { sector, index } => map.mobiles.get(&sector).and_then(|m| m.get(index as usize)),
        ScriptObject::Map(id) => map.map_object(id),
        ScriptObject::Pc(_) | ScriptObject::Carried { .. } | ScriptObject::NpcCarried { .. } => None,
    };
    let none = crate::NpcReaction { base: crate::BASE_REACTION, race: 0, origin: 0, faction: 0, only_reputation: false };
    let Some(object) = object else { return NpcInfo { name: String::new(), male: true, reaction: none, class: 0, dumb: false, overrides: None, healer: Healer::default() } };
    let description = map.prototypes.int(object, "description");
    let name = description.and_then(|d| generic.file(vfs, "mes/description.mes")?.get(d as u32).map(str::to_string)).unwrap_or_default();
    let stats = object.ints("critter_stat_base_idx").or_else(|| map.prototypes.of(object)?.ints("critter_stat_base_idx"));
    let male = stats.as_ref().and_then(|s| s.get(26).copied()).is_none_or(|g| g == 1);
    // Intelligence is stat 4 (the base here: the NPC's effects are not built).
    let dumb = stats.and_then(|s| s.get(4).copied()).is_some_and(|i| i <= 4);
    let class = map.prototypes.int(object, "npc_social_class").unwrap_or(0);
    let array = |field: &str| object.ints(field).or_else(|| map.prototypes.of(object)?.ints(field)).unwrap_or_default();
    let (basic, tech, magic) = (array("critter_basic_skill_idx"), array("critter_tech_skill_idx"), array("critter_spell_tech_idx"));
    let at = |v: &Vec<i32>, i: usize| v.get(i).copied().unwrap_or(0);
    let spells: i32 = (0..16).map(|c| at(&magic, c)).sum();
    let degrees: i32 = (17..25).map(|d| at(&magic, d)).sum();
    let kit = match npc {
        ScriptObject::Mobile { sector, index } => map.npc_inventory(sector, index).is_some_and(|inv| {
            inv.items.iter().any(|i| map.prototypes.get(i.prototype).is_some_and(|p| p.object_type == arcanum_formats::object::TYPE_GENERIC && p.int("generic_flags").unwrap_or(0) & 8 != 0))
        }),
        _ => false,
    };
    let healer = Healer {
        college: at(&magic, 12),
        skills: std::array::from_fn(|k| if k < 12 { at(&basic, k) } else { at(&tech, k - 12) }),
        kit,
        aptitude: ((50 * spells - 55 * degrees) / 10).clamp(-100, 100),
    };
    NpcInfo { name, male, reaction: crate::NpcReaction::of(object, &map.prototypes), class, dumb, overrides: None, healer }
}

/// What one step says, to whom.
pub struct Say<'a> {
    pub npc_net: NetId,
    pub dialog: u32,
    pub pc_name: &'a str,
    pub npc: &'a NpcInfo,
    pub generic: &'a mut GenericLines,
    pub vfs: Option<&'a arcanum_vfs::Vfs>,
    pub rng: &'a mut crate::HostRng,
    pub deltas: Vec<LedgerDelta>,
    /// Lines gone to by `G:` arguments this step.
    pub jumps: usize,
    /// The areas, for directions.
    pub areas: &'a crate::areas::AreaTable,
    /// Where the PC stands.
    pub pc_at: arcanum_formats::map::TileLoc,
}

/// A generic NPC block's kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Block {
    /// `gd_cls`, by the NPC's class (0x418480; override `base / 1000 + 9999`).
    Class,
    /// `gd_rce`, by the PC's race (0x4185f0; override `base / 1000 + 10999`).
    Race,
    /// `gd_npc`, 100 lines (0x418780; override `base / 100 + 11999`).
    Npc,
    /// `gd_npc`, the 10 lines of one direction (0x418780 with `lo, lo + 9`).
    NpcTen,
}

/// Who says a generic NPC line to whom.
pub struct Speaker<'a> {
    pub npc: &'a NpcInfo,
    pub pc_name: &'a str,
    pub pc_female: bool,
    pub pc_race: u8,
}

/// A random line of `lo..=hi` of `path`, names put in (0x4d4550, 0x4cef00, 0x4d43a0). A pick
/// that lands in a gap gives nothing; the original shows "Error! Missing line %d in %s".
#[allow(clippy::too_many_arguments)]
pub fn pick_line(generic: &mut GenericLines, vfs: Option<&arcanum_vfs::Vfs>, rng: &mut crate::HostRng, path: &str, lo: u32, hi: u32, pc_name: &str, npc_name: &str) -> Option<String> {
    let count = generic.file(vfs, path)?.messages.keys().filter(|k| (lo..=hi).contains(k)).count();
    if count == 0 {
        return None;
    }
    let id = lo + rng.range(0, count as i32 - 1) as u32;
    let text = generic.file(vfs, path)?.get(id)?.to_string();
    Some(with_names(&text, pc_name, npc_name))
}

/// Line `number` of the NPC's override dialogue, with its voice (0x418870).
fn override_line(who: &Speaker, number: i32) -> Option<(String, Option<String>)> {
    let (dialog, file) = who.npc.overrides.as_ref()?;
    let entry = file.lines.get(&number)?;
    let text = if who.pc_female && !entry.female_text.is_empty() { &entry.female_text } else { &entry.text };
    Some((with_names(text, who.pc_name, &who.npc.name), voice_path(*dialog, entry.voice(), who.pc_female)))
}

/// A generic NPC line of `block` at `base`: the override first, else a random one of the block.
pub fn npc_line(generic: &mut GenericLines, vfs: Option<&arcanum_vfs::Vfs>, rng: &mut crate::HostRng, who: &Speaker, block: Block, base: u32) -> Option<(String, Option<String>)> {
    let number = match block {
        Block::Class => base as i32 / 1000 + 9999,
        Block::Race => base as i32 / 1000 + 10999,
        Block::Npc | Block::NpcTen => base as i32 / 100 + 11999,
    };
    if let Some(said) = override_line(who, number) {
        return Some(said);
    }
    let npc = who.npc;
    let dumb = if npc.dumb { "dumb_" } else { "" };
    let pair = genders(npc.male, !who.pc_female);
    let (path, offset, len) = match block {
        Block::Class => (format!("mes/gd_cls_{dumb}{pair}.mes"), if npc.dumb { 0 } else { npc.class.max(0) as u32 * 50 }, 50),
        Block::Race => {
            let pc = i32::from(who.pc_race);
            let index = if pc == npc.reaction.race { 0 } else { pc as u32 + 1 };
            (format!("mes/gd_rce_{dumb}{pair}.mes"), if npc.dumb { 0 } else { index * 50 }, 50)
        }
        Block::Npc => (format!("mes/gd_npc_{pair}.mes"), 0, 100),
        Block::NpcTen => (format!("mes/gd_npc_{pair}.mes"), 0, 10),
    };
    let lo = base + offset;
    Some((pick_line(generic, vfs, rng, &path, lo, lo + len - 1, who.pc_name, &npc.name)?, None))
}

impl Say<'_> {
    fn float(&mut self, speaker: NetId, text: String, colour: FloatColour, voice: Option<String>) {
        self.deltas.push(LedgerDelta::FloatText { speaker, text, colour, voice });
    }

    fn line_text<'f>(entry: &'f DialogLine, talk: &Talk) -> &'f str {
        if talk.female() && !entry.female_text.is_empty() { &entry.female_text } else { &entry.text }
    }

    fn pick(&mut self, path: &str, lo: u32, hi: u32) -> Option<String> {
        pick_line(self.generic, self.vfs, self.rng, path, lo, hi, self.pc_name, &self.npc.name)
    }

    fn speaker<'s>(&'s self, talk: &Talk) -> Speaker<'s> {
        Speaker { npc: self.npc, pc_name: self.pc_name, pc_female: talk.female(), pc_race: talk.sheet.race }
    }

    /// The NPC says a generic line of `block` at `base` over its head.
    fn npc_says(&mut self, block: Block, base: u32, talk: &Talk) {
        let (npc, pc_name) = (self.npc, self.pc_name);
        let who = Speaker { npc, pc_name, pc_female: talk.female(), pc_race: talk.sheet.race };
        if let Some((text, voice)) = npc_line(self.generic, self.vfs, self.rng, &who, block, base) {
            self.float(self.npc_net, text, FloatColour::White, voice);
        }
    }

    /// The NPC's answer to the story question (0x41a440): `gd_sto`, its race + 100 × story state;
    /// override `10000 + state`.
    fn story_line(&mut self, talk: &Talk) -> Option<(String, Option<String>)> {
        let state = talk.globals.story_state.max(0);
        if let Some(said) = override_line(&self.speaker(talk), 10000 + state) {
            return Some(said);
        }
        let id = (self.npc.reaction.race.max(0) + state * 100) as u32;
        let path = format!("mes/gd_sto_{}.mes", genders(self.npc.male, !talk.female()));
        Some((self.pick(&path, id, id)?, None))
    }

    /// A PC line of `gd_pc2` (0x4182d0), to the NPC's gender.
    fn pc_line(&mut self, lo: u32, hi: u32, talk: &Talk) -> Option<String> {
        let dumb = if talk.intelligence() <= 4 { "dumb_" } else { "" };
        let path = format!("mes/gd_{dumb}pc2{}.mes", if self.npc.male { 'm' } else { 'f' });
        self.pick(&path, lo, hi)
    }

    /// A PC line of `gd_cls_pc2` by the NPC's class (0x418390).
    fn pc_class_line(&mut self, base: u32, talk: &Talk) -> Option<String> {
        let dumb = if talk.intelligence() <= 4 { "dumb_" } else { "" };
        let path = format!("mes/gd_cls_{dumb}pc2{}.mes", if self.npc.male { 'm' } else { 'f' });
        let lo = base + self.npc.class.max(0) as u32 * 50;
        self.pick(&path, lo, lo + 49)
    }

    /// The generic greeting (0x419260) for `G:` with these arguments: floats it, or says where
    /// the talk goes. The special cases `k` 0-8 are not built.
    pub fn greeting(&mut self, args: &str, talk: &mut Talk) -> Greeting {
        match greeting_args(args)[9] {
            Some(GreetingArg::Line(n)) => return Greeting::Goto(n),
            Some(GreetingArg::Closing(n)) => return Greeting::Closing(n),
            Some(GreetingArg::Off) => return Greeting::Said,
            None => {}
        }
        if let Some((class, base)) = greeting_block(talk.has_met(), crate::level(talk.reaction())) {
            self.npc_says(if class { Block::Class } else { Block::Race }, base, talk);
        }
        Greeting::Said
    }

    /// Shows NPC line `line` and offers its options (0x416840, 0x414e60).
    fn show(&mut self, file: &DialogFile, line: i32, talk: &mut Talk) -> Shown {
        let Some(entry) = file.lines.get(&line).filter(|e| e.is_npc()) else { return Shown::End };
        let closing = talk.run_result(&entry.result);
        let text = Self::line_text(entry, talk).trim();
        match prefix(text) {
            Some('G') => match self.greeting(&text[2..], talk) {
                Greeting::Goto(n) if self.jumps < MAX_JUMPS => {
                    self.jumps += 1;
                    return self.show(file, n, talk);
                }
                Greeting::Goto(_) => return Shown::End,
                Greeting::Closing(n) => return self.show_closing(file, n, talk),
                Greeting::Said => {}
            },
            Some('I') if text.len() == 2 => self.npc_says(Block::Race, 1000, talk),
            _ => {
                let text = with_names(text, self.pc_name, &self.npc.name);
                self.float(self.npc_net, text, FloatColour::White, voice_path(self.dialog, entry.voice(), talk.female()));
            }
        }
        if let Some(closing) = closing {
            return self.show_closing(file, closing, talk);
        }
        let mut shown = Vec::new();
        for (n, option) in options(file, line, talk) {
            // A quest line (0x4c4c00): the option is the table's line for the quest's state.
            let (n, option) = if prefix(option.text.trim()) == Some('Q') {
                let quest = arcanum_formats::dialog::atoi(option.text.trim()[2..].trim().as_bytes());
                match self.quest_line(quest, talk).and_then(|l| Some((l, file.lines.get(&l)?.clone()))) {
                    Some(found) => found,
                    None => continue,
                }
            } else {
                (n, option)
            };
            // Directions with every area known already are left out (0x419e20).
            if matches!(prefix(option.text.trim()), Some('D' | 'X')) && self.unknown_areas(&option.text.trim()[2..], talk).is_empty() {
                continue;
            }
            match self.option_text(&option.text, talk) {
                Some(text) => shown.push(Offered { text, act: OptionAct::Line(n) }),
                None => talk.unbuilt.push(format!("option {}", option.text.trim().chars().take(2).collect::<String>())),
            }
        }
        if shown.is_empty() {
            // 0x414e60: a goodbye that ends the talk.
            let Some(text) = self.pc_line(400, 499, talk) else { return Shown::End };
            shown.push(Offered { text, act: OptionAct::Respond(0) });
        }
        Shown::Line(line, shown)
    }

    /// The text an option shows (0x416c10): as written with names, or a generic line.
    fn option_text(&mut self, text: &str, talk: &Talk) -> Option<String> {
        let text = text.trim();
        match prefix(text) {
            None => Some(with_names(text, self.pc_name, &self.npc.name)),
            // Barter: 1600-1699 with a follower (none yet).
            Some('B') => self.pc_line(300, 399, talk),
            Some('A') => self.pc_class_line(1000, talk),
            // Rumours (gd_cls_pc2 2000) and training (gd_pc2 500): the arguments stay hidden.
            Some('R') => self.pc_class_line(2000, talk),
            Some('T') => self.pc_line(500, 599, talk),
            // Healing (gd_pc2 700) and a skill used on the PC ("[%s Skill]", 900).
            Some('H') => self.pc_line(700, 799, talk),
            Some('U') => self.skill_option(arcanum_formats::dialog::atoi(text[2..].trim().as_bytes()) as usize, talk),
            // Directions and map marks (gd_pc2 1300 / 1400).
            Some('D') => self.pc_line(1300, 1399, talk),
            Some('X') => self.pc_line(1400, 1499, talk),
            // Newspapers (gd_pc2 1900).
            Some('P') => self.pc_line(1900, 1999, talk),
            Some('I') if text.len() == 2 => self.pc_class_line(3000, talk),
            Some('C') => {
                let line = 1700 + talk.globals.story_state.max(0) as u32;
                self.pc_line(line, line, talk)
            }
            Some(_) => {
                let range = generic_range(text)?;
                self.pc_line(range.start, range.end - 1, talk)
            }
        }
    }

    /// The PC chose option `line`: its result, then what its prefix does, then its response
    /// (0x413130, 0x414810).
    fn chosen(&mut self, file: &DialogFile, line: i32, talk: &mut Talk, depth: usize) -> Shown {
        let Some(entry) = file.lines.get(&line).filter(|e| !e.is_npc()) else { return Shown::End };
        // Barter (action 3): the result is not run; the talk ends and the window opens.
        if prefix(entry.text.trim()) == Some('B') {
            return Shown::Barter;
        }
        if let Some(closing) = talk.run_result(&entry.result) {
            return self.show_closing(file, closing, talk);
        }
        // A refusal to follow (0x413a90): the NPC says why (`gd_npc` by reason) and the talk
        // ends (builder's call); a join goes on at the line it names.
        if let Some(r) = talk.follow.refused.take() {
            self.npc_says(Block::Npc, crate::follow::refusal_line(r), talk);
            return Shown::End;
        }
        if let Some(line) = talk.follow.goto.take() {
            return self.respond(file, line, talk, depth);
        }
        let text = entry.text.trim();
        match prefix(text) {
            // An insult (0x4191e0): reaction -10; below 21 the NPC insults back and leaves.
            Some('I') if text.len() == 2 => {
                talk.change_reaction(-10, arcanum_formats::dialog::Change::Add);
                if talk.reaction() < 21 {
                    self.npc_says(Block::Race, 1000, talk);
                    return Shown::End;
                }
            }
            Some('R') => return self.rumours(file, &text[2..], entry.response(), talk),
            Some('T') => return self.training_list(file, &text[2..], entry.response(), talk),
            Some('H') => return self.heal_menu(file, 0, entry.response(), talk),
            Some('U') => {
                let skill = arcanum_formats::dialog::atoi(text[2..].trim().as_bytes()) as usize;
                return self.heal(file, Healing::Skill(skill), entry.response(), talk);
            }
            Some('D' | 'X') => return self.directions(file, line, 0, entry.response(), talk),
            Some('P') => return self.newspaper_menu(file, entry.response(), talk),
            // The story question (0x41a3e0): the answer, then one "thanks" to go on.
            Some('C') => {
                if let Some((said, voice)) = self.story_line(talk) {
                    self.float(self.npc_net, said, FloatColour::White, voice);
                }
                if let Some(thanks) = self.pc_class_line(1000, talk) {
                    return Shown::Line(line, vec![Offered { text: thanks, act: OptionAct::Respond(entry.response()) }]);
                }
            }
            _ => {}
        }
        self.respond(file, entry.response(), talk, depth)
    }

    /// The NPC says a generic line with `%d`s filled.
    fn npc_says_with(&mut self, block: Block, base: u32, numbers: &[i32], talk: &Talk) {
        let who = Speaker { npc: self.npc, pc_name: self.pc_name, pc_female: talk.female(), pc_race: talk.sheet.race };
        if let Some((text, voice)) = npc_line(self.generic, self.vfs, self.rng, &who, block, base) {
            let text = crate::barter::with_numbers(&text, numbers);
            self.float(self.npc_net, text, FloatColour::White, voice);
        }
    }

    /// The NPC says a generic line and the PC gets one option back to `ret`.
    #[allow(clippy::too_many_arguments)]
    fn say_then_back(&mut self, file: &DialogFile, block: Block, base: u32, numbers: &[i32], option: Option<String>, ret: i32, talk: &mut Talk) -> Shown {
        self.npc_says_with(block, base, numbers, talk);
        match option {
            Some(text) => Shown::Line(0, vec![Offered { text, act: OptionAct::Respond(ret) }]),
            None => self.back(file, ret, talk),
        }
    }

    /// Without a line for the option: straight on to `ret`.
    fn back(&mut self, file: &DialogFile, ret: i32, talk: &mut Talk) -> Shown {
        self.respond(file, ret, talk, 0)
    }

    /// Rumours (`R:$price, list`, 0x418fc0): one the PC does not know and nobody quelled, at
    /// random; none, `gd_cls` 7000. A price asks first (0x418a40).
    fn rumours(&mut self, file: &DialogFile, args: &str, ret: i32, talk: &mut Talk) -> Shown {
        let (price, list) = priced_list(args);
        let left: Vec<u32> = list.into_iter().filter_map(|n| u32::try_from(n).ok()).filter(|n| !talk.pc.rumours.contains(n) && !talk.globals.quelled.contains(n)).collect();
        if left.is_empty() {
            let more = self.pc_line(600, 699, talk);
            return self.say_then_back(file, Block::Class, 7000, &[], more, ret, talk);
        }
        let rumour = left[self.rng.range(0, left.len() as i32 - 1) as usize];
        if price < 1 { self.tell_rumour(file, rumour, ret, talk) } else { self.ask_price(file, price, Service::Rumour(rumour), ret, talk) }
    }

    /// Tells rumour `n` (0x4190e0, 0x4c5700): line `20 n + 1 +` the NPC's class of
    /// `game_rd_npc_*` (else `20 n + 1`); the PC knows it now. Then one "thanks".
    fn tell_rumour(&mut self, file: &DialogFile, n: u32, ret: i32, talk: &mut Talk) -> Shown {
        let path = format!("mes/game_rd_npc_{}.mes", genders(self.npc.male, !talk.female()));
        let class = self.npc.class.max(0) as u32;
        let text = [n * 20 + 1 + class, n * 20 + 1].into_iter().find_map(|id| self.generic.file(self.vfs, &path)?.get(id).map(str::to_string));
        if let Some(text) = text {
            self.float(self.npc_net, with_names(&text, self.pc_name, &self.npc.name), FloatColour::White, None);
        }
        talk.pc.rumours.insert(n);
        let thanks = self.pc_class_line(1000, talk);
        self.say_then_back_quiet(file, thanks, ret, talk)
    }

    fn say_then_back_quiet(&mut self, file: &DialogFile, option: Option<String>, ret: i32, talk: &mut Talk) -> Shown {
        match option {
            Some(text) => Shown::Line(0, vec![Offered { text, act: OptionAct::Respond(ret) }]),
            None => self.back(file, ret, talk),
        }
    }

    /// A training list (`T:skills`, 0x418ca0): `gd_cls` 3000, a skill name per skill, then a
    /// "forget it" back.
    fn training_list(&mut self, file: &DialogFile, args: &str, ret: i32, talk: &mut Talk) -> Shown {
        self.npc_says_with(Block::Class, 3000, &[], talk);
        let mut options = Vec::new();
        for skill in number_list(args).into_iter().filter_map(|k| usize::try_from(k).ok()).filter(|&k| k < arcanum_rules::SKILLS) {
            if let Some(name) = self.generic.file(self.vfs, "mes/skill.mes").and_then(|m| m.get(skill as u32)).map(str::to_string) {
                options.push(Offered { text: name, act: OptionAct::Train { skill, ret } });
            }
        }
        if let Some(back) = self.pc_line(800, 899, talk) {
            options.push(Offered { text: back, act: OptionAct::Respond(ret) });
        }
        if options.is_empty() { self.back(file, ret, talk) } else { Shown::Line(0, options) }
    }

    /// Training `skill` (0x418de0): trained already, `gd_cls` 4000; below the level for
    /// Apprentice ([`RANK_LEVELS`]), 5000; else the price.
    fn train(&mut self, file: &DialogFile, skill: usize, ret: i32, talk: &mut Talk) -> Shown {
        let more = self.pc_line(600, 699, talk);
        if talk.sheet.training[skill] != 0 {
            return self.say_then_back(file, Block::Class, 4000, &[], more, ret, talk);
        }
        if talk.rules.skill_level(talk.sheet, skill) < RANK_LEVELS[APPRENTICE as usize] {
            return self.say_then_back(file, Block::Class, 5000, &[], more, ret, talk);
        }
        self.ask_price(file, TRAINING_PRICE, Service::Train(skill), ret, talk)
    }

    /// "[%s Skill]" for skill `k` (gd_pc2 900, 0x419a00).
    fn skill_option(&mut self, k: usize, talk: &Talk) -> Option<String> {
        let name = self.generic.file(self.vfs, "mes/skill.mes")?.get(k as u32)?.to_string();
        Some(self.pc_line(900, 999, talk)?.replace("%s", &name))
    }

    /// "[%s Spell]" for spell `spell` (gd_pc2 1000, 0x419ac0).
    fn spell_option(&mut self, spell: u32, talk: &Talk) -> Option<String> {
        let name = self.generic.file(self.vfs, "mes/spell.mes")?.get(spell)?.to_string();
        Some(self.pc_line(1000, 1099, talk)?.replace("%s", &name))
    }

    /// The healing menu (0x419830): `gd_cls` 14000, then by page what the NPC can do, and a
    /// "forget it" (800) back to `ret`. Page 0: the Heal skill (with a kit), "[Magickal
    /// Healing]" (1100, 1 spell or more), "[Magickal Poison Healing]" (1200, 2 or more),
    /// Resurrect (all 5); page 1: Minor Healing (1), Major Healing (3); page 2: Halt Poison (2).
    /// Nothing asks whether the PC is hurt.
    fn heal_menu(&mut self, file: &DialogFile, page: u8, ret: i32, talk: &mut Talk) -> Shown {
        self.npc_says(Block::Class, 14000, talk);
        let h = self.npc.healer;
        let mut options = Vec::new();
        let heal = |healing| OptionAct::Heal { healing, ret };
        match page {
            0 => {
                if h.kit && let Some(text) = self.skill_option(HEAL_SKILL, talk) {
                    options.push(Offered { text, act: heal(Healing::Skill(HEAL_SKILL)) });
                }
                if h.college >= 1 && let Some(text) = self.pc_line(1100, 1199, talk) {
                    options.push(Offered { text, act: OptionAct::HealPage { page: 1, ret } });
                }
                if h.college >= 2 && let Some(text) = self.pc_line(1200, 1299, talk) {
                    options.push(Offered { text, act: OptionAct::HealPage { page: 2, ret } });
                }
                if h.college == 5 && talk.party_dead && let Some(text) = self.spell_option(RESURRECT, talk) {
                    options.push(Offered { text, act: heal(Healing::Spell(RESURRECT)) });
                }
            }
            1 => {
                for (spell, needs) in [(MINOR_HEALING, 1), (MAJOR_HEALING, 3)] {
                    if h.college >= needs && let Some(text) = self.spell_option(spell, talk) {
                        options.push(Offered { text, act: heal(Healing::Spell(spell)) });
                    }
                }
            }
            _ => {
                if h.college >= 2 && let Some(text) = self.spell_option(HALT_POISON, talk) {
                    options.push(Offered { text, act: heal(Healing::Spell(HALT_POISON)) });
                }
            }
        }
        if let Some(back) = self.pc_line(800, 899, talk) {
            options.push(Offered { text: back, act: OptionAct::Respond(ret) });
        }
        if options.is_empty() { self.back(file, ret, talk) } else { Shown::Line(0, options) }
    }

    /// A healing chosen (0x419b50 / 0x419c30): free from the PC's follower, else priced: a
    /// skill `(level + 2) × (rank + 1)` (0x4c62d0), a spell 100 (0x4b1740).
    pub fn heal(&mut self, file: &DialogFile, healing: Healing, ret: i32, talk: &mut Talk) -> Shown {
        if talk.follow.mine {
            return self.perform_heal(file, healing, ret, talk);
        }
        let base = match healing {
            Healing::Skill(k) => {
                let (level, rank) = self.npc.healer.skill(k);
                (level + 2) * (rank + 1)
            }
            Healing::Spell(_) => SPELL_PRICE,
        };
        self.ask_price(file, base, Service::Heal(healing), ret, talk)
    }

    /// The healing done (0x419cb0 / 0x419d50), then `gd_cls` 15000 and one "thanks"
    /// (`gd_cls_pc2` 1000) back to `ret`.
    /// - The Heal skill (0x4c7160 case 10; a healer that follows nobody never fails): its level,
    ///   × 1.5 when trained, at least 1; a Master heals all of it, others 1 to it at random.
    /// - Minor / Major Healing: [`spell_heal`] by the NPC's aptitude; Halt Poison: 50 poison.
    /// - A Master's Heal cures injuries (builder's call: 0x4c7160 case 10 clears `critter_flags`
    ///   0x80-0x400; the rank test not traced).
    /// - Resurrect (offered only while a PC of the party is dead): the dead PCs get up
    ///   ([`crate::Resurrections`]).
    /// - Not built: other skills, the kit's charges.
    fn perform_heal(&mut self, file: &DialogFile, healing: Healing, ret: i32, talk: &mut Talk) -> Shown {
        match healing {
            Healing::Skill(HEAL_SKILL) => {
                let (level, rank) = self.npc.healer.skill(HEAL_SKILL);
                let mut amount = if rank != 0 { level * 3 / 2 } else { level };
                amount = amount.max(1);
                if rank != 3 {
                    amount = self.rng.range(1, amount);
                }
                talk.sheet_changes.heal += amount;
                talk.sheet_changes.cure_injuries |= rank == 3;
            }
            Healing::Spell(HALT_POISON) => talk.sheet_changes.cure_poison += 50,
            Healing::Spell(RESURRECT) => talk.resurrect = true,
            Healing::Spell(spell) => talk.sheet_changes.heal += spell_heal(spell, self.npc.healer.aptitude),
            Healing::Skill(_) => {}
        }
        let thanks = self.pc_class_line(1000, talk);
        self.say_then_back(file, Block::Class, 15000, &[], thanks, ret, talk)
    }

    /// The newspaper menu (`P:`, 0x41a520): `gd_npc` 4200, then buy today's paper (`gd_pc2`
    /// 2200), today's headline (2000), older papers (2100) and "forget it" (800) back to `ret`.
    fn newspaper_menu(&mut self, file: &DialogFile, ret: i32, talk: &mut Talk) -> Shown {
        self.npc_says(Block::Npc, 4200, talk);
        let today = talk.globals.newspapers.today();
        let mut options = Vec::new();
        for (lo, act) in [(2200, OptionAct::BuyPaper { paper: today, ret }), (2000, OptionAct::Headline { ret }), (2100, OptionAct::OldPapers { ret }), (800, OptionAct::Respond(ret))] {
            if let Some(text) = self.pc_line(lo, lo + 99, talk) {
                options.push(Offered { text, act });
            }
        }
        if options.is_empty() { self.back(file, ret, talk) } else { Shown::Line(0, options) }
    }

    /// Paper `n`'s headline ([`crate::newspaper::headline`]).
    fn paper_headline(&mut self, n: i32) -> Option<String> {
        let entry = self.generic.file(self.vfs, crate::newspaper::NEWSPAPER_MES)?.get(u32::try_from(n).ok()?)?.to_string();
        Some(crate::newspaper::headline(&entry))
    }

    /// Today's headline (0x41a620): the headline and a `gd_npc` 4300 line, then yes (`gd_pc2`
    /// 1-99) to buy it or no (100-199) back.
    fn headline(&mut self, file: &DialogFile, ret: i32, talk: &mut Talk) -> Shown {
        let today = talk.globals.newspapers.today();
        let who = Speaker { npc: self.npc, pc_name: self.pc_name, pc_female: talk.female(), pc_race: talk.sheet.race };
        let ask = npc_line(self.generic, self.vfs, self.rng, &who, Block::Npc, 4300).map(|l| l.0);
        let said = [self.paper_headline(today), ask].into_iter().flatten().collect::<Vec<_>>().join(" ");
        if !said.is_empty() {
            self.float(self.npc_net, said, FloatColour::White, None);
        }
        let mut options = Vec::new();
        for (lo, act) in [(1, OptionAct::BuyPaper { paper: today, ret }), (100, OptionAct::Respond(ret))] {
            if let Some(text) = self.pc_line(lo, lo + 99, talk) {
                options.push(Offered { text, act });
            }
        }
        if options.is_empty() { self.back(file, ret, talk) } else { Shown::Line(0, options) }
    }

    /// The older papers (0x41a700): the queue's first four that are set. None: `gd_npc` 4400 and
    /// a "more" (`gd_pc2` 600) back; else 4500 and one option per paper, its headline, and
    /// "forget it" (800; builder's call: the original offers no way back).
    fn old_papers(&mut self, file: &DialogFile, ret: i32, talk: &mut Talk) -> Shown {
        let papers: Vec<i32> = talk.globals.newspapers.queue.iter().take(crate::newspaper::TODAY).copied().filter(|&n| n != -1).collect();
        if papers.is_empty() {
            let more = self.pc_line(600, 699, talk);
            return self.say_then_back(file, Block::Npc, 4400, &[], more, ret, talk);
        }
        self.npc_says(Block::Npc, 4500, talk);
        let mut options: Vec<Offered> = papers.into_iter().filter_map(|paper| Some(Offered { text: self.paper_headline(paper)?, act: OptionAct::BuyPaper { paper, ret } })).collect();
        if let Some(back) = self.pc_line(800, 899, talk) {
            options.push(Offered { text: back, act: OptionAct::Respond(ret) });
        }
        Shown::Line(0, options)
    }

    /// The price dialogue (0x418a40): `base` × the reaction factor, at least 2; the NPC names it
    /// (`gd_cls` 1000) and the PC says yes (`gd_pc2` 1-99) or no (100-199).
    fn ask_price(&mut self, file: &DialogFile, base: i32, service: Service, ret: i32, talk: &mut Talk) -> Shown {
        let price = (base * crate::barter::reaction_factor(talk.reaction()) / 100).max(2);
        self.npc_says_with(Block::Class, 1000, &[price], talk);
        let mut options = Vec::new();
        if let Some(yes) = self.pc_line(1, 99, talk) {
            options.push(Offered { text: yes, act: OptionAct::Pay { price, service, ret } });
        }
        if let Some(no) = self.pc_line(100, 199, talk) {
            options.push(Offered { text: no, act: OptionAct::Respond(ret) });
        }
        if options.is_empty() { self.back(file, ret, talk) } else { Shown::Line(0, options) }
    }

    /// Paying (0x418b30): short of gold, `gd_cls` 2000; else the gold goes and the service is
    /// done.
    fn pay(&mut self, file: &DialogFile, price: i32, service: Service, ret: i32, talk: &mut Talk) -> Shown {
        if talk.gold() < price {
            let more = self.pc_line(600, 699, talk);
            return self.say_then_back(file, Block::Class, 2000, &[price], more, ret, talk);
        }
        talk.change_gold(-price);
        match service {
            Service::Rumour(n) => self.tell_rumour(file, n, ret, talk),
            Service::Directions { area, mark } => self.direct(file, area, mark, ret, talk),
            Service::Heal(healing) => self.perform_heal(file, healing, ret, talk),
            Service::Newspaper(paper) => {
                // The paper (basic prototype 14064, 0x4bf210) with its number as its first line.
                let item = crate::CarriedItem { prototype: NEWSPAPER_PROTOTYPE, location: 0, quantity: 0, keys: vec![], written: paper, wear: Default::default() };
                let (carry, aptitude) = (talk.rules.derived(talk.sheet).carry_weight, talk.rules.aptitude(talk.sheet));
                let mut mine = talk.inventory.clone();
                if mine.insert_item(item, talk.item_info, carry, aptitude).is_ok() {
                    *talk.inventory = mine;
                } else {
                    // No room: the gold back (builder's call; the original drops it at the PC's feet).
                    talk.change_gold(price);
                }
                let thanks = self.pc_class_line(1000, talk);
                self.say_then_back(file, Block::Class, 15000, &[], thanks, ret, talk)
            }
            Service::Train(skill) => {
                talk.sheet_changes.training.push((skill, APPRENTICE));
                let thanks = self.pc_line(1000, 1099, talk);
                self.say_then_back(file, Block::Class, 6000, &[], thanks, ret, talk)
            }
        }
    }

    /// The areas of a directions option's list the PC does not know yet.
    fn unknown_areas(&self, args: &str, talk: &Talk) -> Vec<u32> {
        let (_, list) = priced_list(args);
        list.into_iter().filter_map(|a| u32::try_from(a).ok()).filter(|a| self.areas.areas.contains_key(a) && !talk.pc.areas.contains(a)).collect()
    }

    /// A directions list (0x419e70): `gd_npc` 100 (`D:`) or 500 (`X:`), then up to four unknown
    /// areas from `offset`, a "more" (`gd_pc2` 600) or a "back" (800).
    fn directions(&mut self, file: &DialogFile, option: i32, offset: usize, ret: i32, talk: &mut Talk) -> Shown {
        let Some(text) = file.lines.get(&option).map(|e| e.text.trim().to_string()) else { return self.back(file, ret, talk) };
        let mark = prefix(&text) == Some('X');
        let (price, _) = priced_list(&text[2..]);
        let unknown = self.unknown_areas(&text[2..], talk);
        self.npc_says_with(Block::Npc, if mark { 500 } else { 100 }, &[], talk);
        let mut options: Vec<Offered> = unknown.iter().skip(offset).take(4).filter_map(|&area| Some(Offered { text: self.areas.areas.get(&area)?.name.clone(), act: OptionAct::Direct { area, price, mark, ret } })).collect();
        let (more, act) = if unknown.len() > offset + 4 { (self.pc_line(600, 699, talk), OptionAct::MoreDirections { option, offset: offset + 4, ret }) } else { (self.pc_line(800, 899, talk), OptionAct::Respond(ret)) };
        if let Some(more) = more {
            options.push(Offered { text: more, act });
        }
        if options.is_empty() { self.back(file, ret, talk) } else { Shown::Line(0, options) }
    }

    /// The way to an area (0x41a150 / 0x41a290): the direction (0-7) and the distance
    /// (0x4b96f0 / 3168) pick a `gd_npc` row: `D:` 200 below 2, 300 below 100, else 400; `X:`
    /// 600 below 2, else 700 with the distance, and the area is known now. Then a "thanks".
    pub fn direct(&mut self, file: &DialogFile, area: u32, mark: bool, ret: i32, talk: &mut Talk) -> Shown {
        let Some(target) = self.areas.areas.get(&area).map(|a| a.at) else { return self.back(file, ret, talk) };
        let rotation = crate::areas::direction(self.pc_at, target);
        let distance = ((f64::from(target.x) - f64::from(self.pc_at.x)).hypot(f64::from(target.y) - f64::from(self.pc_at.y)) / 3168.0) as i32;
        let base = match (mark, distance) {
            (false, d) if d < 2 => 200,
            (false, d) if d < 100 => 300,
            (false, _) => 400,
            (true, d) if d < 2 => 600,
            (true, _) => 700,
        };
        self.npc_says_with(Block::NpcTen, base + rotation * 10, &[distance], talk);
        if mark {
            talk.pc.areas.insert(area);
        }
        let thanks = self.pc_class_line(1000, talk);
        self.say_then_back_quiet(file, thanks, ret, talk)
    }

    /// Quest `n`'s option line for this PC (0x4c4c00).
    fn quest_line(&mut self, n: i32, talk: &Talk) -> Option<i32> {
        let row = self.generic.file(self.vfs, GAME_QUEST)?.get(u32::try_from(n).ok()?)?.to_string();
        quest_option(&row, talk.pc.quest(n), talk.intelligence(), talk.reaction())
    }

    /// Follows a response: a line, 0 the end, `-n` line `n` (a PC option taken as chosen).
    fn respond(&mut self, file: &DialogFile, response: i32, talk: &mut Talk, depth: usize) -> Shown {
        match response {
            0 => Shown::End,
            n if n > 0 => self.show(file, n, talk),
            n => match file.lines.get(&-n) {
                Some(target) if target.is_npc() => self.show(file, -n, talk),
                Some(_) if depth < MAX_JUMPS => self.chosen(file, -n, talk, depth + 1),
                _ => Shown::End,
            },
        }
    }

    fn show_closing(&mut self, file: &DialogFile, line: i32, talk: &mut Talk) -> Shown {
        if let Some(last) = file.lines.get(&line) {
            let text = with_names(Self::line_text(last, talk), self.pc_name, &self.npc.name);
            self.float(self.npc_net, text, FloatColour::White, voice_path(self.dialog, last.voice(), talk.female()));
        }
        Shown::End
    }
}

/// Where a `G:` line leaves the talk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Greeting {
    /// The greeting was said (or nothing); the line's options follow.
    Said,
    /// Go to this NPC line.
    Goto(i32),
    /// Close with this line.
    Closing(i32),
}

#[cfg(test)]
mod tests {
    use super::*;

    static NO_REPUTATIONS: std::sync::LazyLock<crate::ReputationEffects> = std::sync::LazyLock::new(Default::default);
    static NO_PROTOTYPES: std::sync::LazyLock<arcanum_formats::object::Prototypes> = std::sync::LazyLock::new(Default::default);

    fn talk_with<'a>(rules: &'a RulesData, sheet: &'a CharacterSheet, inventory: &'a mut Inventory, pc: &'a mut PcScriptState, globals: &'a mut ScriptGlobals, npc: &'a mut Attachment) -> Talk<'a> {
        let npc_reaction = crate::NpcReaction { base: 50, race: 0, origin: 0, faction: 0, only_reputation: false };
        Talk {
            attack: false,
            party_dead: false,
            resurrect: false,
            follow: Default::default(),
            rules,
            sheet,
            inventory,
            pc,
            globals,
            npc,
            met: false,
            npc_key: "npc".into(),
            npc_reaction,
            reputations: &NO_REPUTATIONS,
            gold_changed: false,
            unbuilt: Vec::new(),
            sheet_changes: SheetChanges::default(),
            pc_area: 0,
            prototypes: &NO_PROTOTYPES,
            npc_flags: 0,
            npc_inventory: Some(Inventory::default()),
            npc_stock: None,
            npc_inventory_changed: false,
            xp_quest: None,
            item_info: &|_| None,
        }
    }

    #[test]
    fn join_wait_rejoin_spread_out_and_leave() {
        use crate::follow::{FORCED, Refusal, SPREAD_OUT, WAITING};
        let rules = RulesData::default();
        let sheet = CharacterSheet::default();
        let (mut inventory, mut pc, mut globals, mut npc) = Default::default();
        let mut talk = talk_with(&rules, &sheet, &mut inventory, &mut pc, &mut globals, &mut npc);
        talk.follow.npc_level = 1;
        assert!(talk.passes_test("fo 0") && !talk.passes_test("fo 1"));
        talk.run_result("jo 0 74");
        assert!(talk.follow.mine && talk.passes_test("fo 1, wa 0"), "joined");
        assert_eq!((talk.follow.goto, talk.follow.followers), (Some(74), 1));
        talk.run_result("wa");
        assert!(talk.npc_flags & WAITING != 0 && talk.passes_test("wa 1, fo 1"), "waiting, still its leader");
        assert_eq!(talk.follow.followers, 0, "not counted while waiting");
        talk.follow.goto = None;
        talk.run_result("uw 0 9, so");
        assert_eq!((talk.npc_flags & (WAITING | SPREAD_OUT), talk.follow.goto, talk.follow.followers), (SPREAD_OUT, Some(9), 1));
        talk.run_result("sc, lv");
        assert!(!talk.follow.mine && talk.npc_flags == 0 && talk.passes_test("fo 0"));
        // Charisma 8 leads 2: a third is refused, unless forced.
        talk.follow.followers = 2;
        talk.run_result("jo 0 74");
        assert_eq!(talk.follow.refused.take(), Some(Refusal::TooMany));
        talk.run_result("jo 1 74");
        assert!(talk.follow.mine && talk.npc_flags & FORCED != 0 && talk.follow.followers == 2, "forced: not counted");
        // Waiting: 2 hours (npc_wait 2, no Persuasion).
        talk.follow.npc_wait = crate::follow::DEFAULT_NPC_WAIT;
        talk.run_result("wa");
        assert_eq!(talk.follow.waits, Some(Some(7_200_000)));
        // Gold: the followers' counts for $$, and pays after the PC's.
        talk.change_gold(30);
        talk.follow.followers_gold = vec![((arcanum_formats::map::SectorId::new(1, 1), 0), 50), ((arcanum_formats::map::SectorId::new(1, 1), 1), 50)];
        assert!(talk.passes_test("$$ 120") && !talk.passes_test("$$ 131"));
        talk.run_result("$$ -100");
        assert_eq!(talk.gold(), 0);
        assert_eq!(talk.follow.followers_gold.iter().map(|g| g.1).collect::<Vec<_>>(), [0, 30], "30 from the PC, 50 and 20 from the followers");
        assert!(talk.unbuilt.is_empty(), "{:?}", talk.unbuilt);
    }

    #[test]
    fn options_follow_their_line_and_pass_gender_intelligence_and_tests() {
        let file = DialogFile::parse(
            b"{1}{Hello.}{Hello, lady.}{}{1}{}{}\n\
              {2}{For men.}{1}{5}{}{10}{}\n\
              {3}{For women.}{0}{5}{}{10}{}\n\
              {4}{Clever.}{}{9}{}{10}{}\n\
              {5}{Me dumb.}{}{-4}{}{10}{}\n\
              {6}{Flag 7.}{}{5}{gf 7 1}{10}{}\n\
              {7}{Gold.}{}{1}{$$ 50}{10}{}\n\
              {8}{Next NPC line.}{}{}{}{}{}\n\
              {9}{Not shown.}{}{5}{}{10}{}",
        );
        let rules = RulesData::default();
        let sheet = CharacterSheet { gender: arcanum_rules::MALE, ..Default::default() };
        let (mut inventory, mut pc, mut globals, mut npc) = Default::default();
        let mut talk = talk_with(&rules, &sheet, &mut inventory, &mut pc, &mut globals, &mut npc);
        let iq = talk.intelligence();
        assert!((5..9).contains(&iq), "{iq}");
        let shown: Vec<i32> = options(&file, 1, &mut talk).into_iter().map(|(n, _)| n).collect();
        assert_eq!(shown, [2], "male, not clever, not dumb, no flag, no gold");
        talk.globals.set_flag(7, true);
        talk.change_gold(60);
        let shown: Vec<i32> = options(&file, 1, &mut talk).into_iter().map(|(n, _)| n).collect();
        assert_eq!(shown, [2, 6, 7]);
    }

    #[test]
    fn results_change_flags_gold_and_quests_and_skip_what_is_not_built() {
        let rules = RulesData::default();
        let sheet = CharacterSheet::default();
        let (mut inventory, mut pc, mut globals, mut npc): (Inventory, PcScriptState, ScriptGlobals, Attachment) = Default::default();
        let mut talk = talk_with(&rules, &sheet, &mut inventory, &mut pc, &mut globals, &mut npc);
        talk.change_gold(100);
        assert_eq!(talk.run_result("$$ -30, re +5, lf 2 1, lc 1 4, qu 12 2, gf 40 1, ru 7"), None);
        assert_eq!(talk.gold(), 70);
        assert!(talk.unbuilt.is_empty(), "{:?}", talk.unbuilt);
        assert_eq!(talk.npc.flags, 1 << 2);
        assert_eq!(talk.npc.counters[1], 4);
        assert!(talk.passes_test("qu 12 2, qa 12 1, qb 12 2, lf 2 1, lc 1 4, gf 40 1, ru 7"));
        assert!(!talk.passes_test("ru -7"));
        assert_eq!(talk.run_result("gv 3 9, fl 120, gv 4 1"), Some(120), "fl ends the result");
        assert_eq!((talk.globals.var(3), talk.globals.var(4)), (9, 0));
        assert!(!talk.passes_test("tr 1 1") && talk.passes_test("tr 1 0"), "no training in skill 1");
        assert!(talk.unbuilt.is_empty(), "every test opcode is built: {:?}", talk.unbuilt);
    }

    #[test]
    fn reaction_starts_at_the_base_plus_looks_and_changes_are_stored() {
        let rules = RulesData::default();
        let sheet = CharacterSheet::default();
        let (mut inventory, mut pc, mut globals, mut npc): (Inventory, PcScriptState, ScriptGlobals, Attachment) = Default::default();
        let mut talk = talk_with(&rules, &sheet, &mut inventory, &mut pc, &mut globals, &mut npc);
        let looks = rules.derived(&sheet).reaction;
        assert_eq!(talk.reaction(), 50 + looks, "human to human: no racial term");
        assert!(!talk.passes_test("me 1"), "not met");
        talk.run_result("re +5");
        assert_eq!(talk.reaction(), 55 + looks);
        assert!(talk.passes_test("me 1"), "a stored reaction: met");
        talk.run_result("re 61");
        assert_eq!(talk.reaction(), 61, "set");
        assert!(talk.passes_test("re 61") && !talk.passes_test("re 62") && talk.passes_test("re -61"));
        talk.run_result("re >70");
        assert_eq!(talk.reaction(), 70);
        talk.run_result("re >60, re <65");
        assert_eq!(talk.reaction(), 65);
        assert_eq!(talk.pc.reactions["npc"], 65 - looks, "the unmodified part is stored");
    }

    #[test]
    fn names_go_into_the_text() {
        assert_eq!(with_names("Welcome, @PCname@. I am @npcname@ @ home.", "Virgil", "Kietzel"), "Welcome, Virgil. I am Kietzel @ home.");
        assert_eq!(voice_path(1990, 3, true).as_deref(), Some("sound/speech/01990/v3_f.mp3"));
        assert_eq!(voice_path(1990, 0, true), None);
        assert!(is_prefixed("E:") && is_prefixed("T:5,6") && !is_prefixed("Every day."));
        assert_eq!(generic_range("E:"), Some(400..500));
        assert_eq!(generic_range("B:"), None);
    }

    fn generic_with(files: &[(&str, &str)]) -> GenericLines {
        let mut generic = GenericLines::default();
        for (path, text) in files {
            generic.0.insert(path.to_string(), Some(MesFile::parse(text.as_bytes()).unwrap()));
        }
        generic
    }

    fn npc(class: i32) -> NpcInfo {
        let reaction = crate::NpcReaction { base: 50, race: 0, origin: 0, faction: 0, only_reputation: false };
        NpcInfo { name: "Kietzel".into(), male: true, reaction, class, dumb: false, overrides: None, healer: Healer::default() }
    }

    fn floats(deltas: &[LedgerDelta]) -> Vec<(String, FloatColour)> {
        deltas.iter().filter_map(|d| match d { LedgerDelta::FloatText { text, colour, .. } => Some((text.clone(), *colour)), _ => None }).collect()
    }

    #[test]
    fn greeting_arguments_name_a_case_and_a_line() {
        let args = greeting_args(" 1 10, 2 10, 4 fl 730, 8 0, 9 2");
        assert_eq!(args[1], Some(GreetingArg::Line(10)));
        assert_eq!(args[3], None);
        assert_eq!(args[4], Some(GreetingArg::Closing(730)));
        assert_eq!(args[8], Some(GreetingArg::Off));
        assert_eq!(args[9], Some(GreetingArg::Line(2)));
        assert_eq!(greeting_args("1 3,2 6")[2], Some(GreetingArg::Line(6)));
        assert_eq!(greeting_block(false, 0), Some((true, 8000)));
        assert_eq!(greeting_block(false, 2), Some((true, 9000)));
        assert_eq!(greeting_block(true, 3), Some((true, 13000)));
        assert_eq!(greeting_block(false, 5), Some((false, 2000)));
        assert_eq!(greeting_block(true, 6), Some((false, 5000)));
        assert_eq!(greeting_block(true, 7), None);
        assert_eq!(prefix(" e:"), Some('E'));
    }

    #[test]
    fn a_g_line_greets_by_class_and_reaction_then_its_options_follow() {
        let file = DialogFile::parse(
            b"{1}{G:}{}{}{}{}{}\n\
              {2}{A:}{}{1}{}{0}{}\n\
              {3}{B:}{}{1}{}{0}{}\n\
              {10}{G: 9 20}{}{}{}{}{}\n\
              {20}{Over here.}{}{}{}{}{}\n\
              {30}{G: 9 fl 31}{}{}{}{}{}\n\
              {31}{Begone.}{}{}{}{}{}",
        );
        let rules = RulesData::default();
        let sheet = CharacterSheet { gender: arcanum_rules::MALE, ..Default::default() };
        let (mut inventory, mut pc, mut globals, mut attachment) = Default::default();
        let mut talk = talk_with(&rules, &sheet, &mut inventory, &mut pc, &mut globals, &mut attachment);
        assert_eq!(crate::level(talk.reaction()), 3, "a neutral first meeting");
        let mut generic = generic_with(&[
            ("mes/gd_cls_m2m.mes", "{10000}{Noble hello.}{10050}{Priest hello, @pcname@.}{13050}{Priest again.}"),
            ("mes/gd_cls_pc2m.mes", "{1050}{Thank you, Father.}"),
            ("mes/gd_pc2m.mes", "{400}{Farewell.}"),
        ]);
        let info = npc(1);
        let mut rng = crate::HostRng::default();
        let mut say = Say { npc_net: NetId(7), dialog: 1, pc_name: "Vel", npc: &info, generic: &mut generic, vfs: None, rng: &mut rng, deltas: Vec::new(), jumps: 0, areas: &crate::areas::NO_AREAS, pc_at: arcanum_formats::map::TileLoc { x: 0, y: 0 } };
        let Shown::Line(1, options) = say.show(&file, 1, &mut talk) else { panic!("options follow") };
        assert_eq!(floats(&say.deltas), [("Priest hello, Vel.".to_string(), FloatColour::White)]);
        assert_eq!(options, [Offered { text: "Thank you, Father.".into(), act: OptionAct::Line(2) }], "A: thanks; B: not built");
        assert_eq!(talk.unbuilt, ["option B:"]);
        talk.met = true;
        say.deltas.clear();
        say.show(&file, 1, &mut talk);
        assert_eq!(floats(&say.deltas)[0].0, "Priest again.");
        say.deltas.clear();
        let Shown::Line(20, options) = say.show(&file, 10, &mut talk) else { panic!("9 20 goes to line 20") };
        assert_eq!(floats(&say.deltas)[0].0, "Over here.");
        assert_eq!(options, [Offered { text: "Farewell.".into(), act: OptionAct::Respond(0) }], "no options: a goodbye that ends");
        say.deltas.clear();
        assert!(matches!(say.show(&file, 30, &mut talk), Shown::End));
        assert_eq!(floats(&say.deltas)[0].0, "Begone.");
    }

    #[test]
    fn overrides_insults_and_the_story_question() {
        let file = DialogFile::parse(
            b"{1}{Well?}{}{}{}{}{}\n\
              {2}{I:}{}{1}{}{1}{}\n\
              {3}{C:}{}{1}{}{1}{}\n\
              {10}{I:}{}{}{}{}{}",
        );
        let rules = RulesData::default();
        let sheet = CharacterSheet { gender: arcanum_rules::MALE, ..Default::default() };
        let (mut inventory, mut pc, mut globals, mut attachment): (Inventory, PcScriptState, ScriptGlobals, Attachment) = Default::default();
        globals.story_state = 2;
        let mut talk = talk_with(&rules, &sheet, &mut inventory, &mut pc, &mut globals, &mut attachment);
        let mut generic = generic_with(&[
            ("mes/gd_cls_pc2m.mes", "{3000}{You stink.}{1000}{Thanks.}"),
            ("mes/gd_pc2m.mes", "{1702}{What news?}"),
            ("mes/gd_rce_m2m.mes", "{1000}{Same to you.}"),
            ("mes/gd_sto_m2m.mes", "{200}{The war goes badly.}"),
        ]);
        let mut info = npc(0);
        let overrides = DialogFile::parse(b"{11000}{Mind your tongue, @pcname@.}{}{}{4}{}{}");
        info.overrides = Some((1848, std::sync::Arc::new(overrides)));
        let mut rng = crate::HostRng::default();
        let mut say = Say { npc_net: NetId(7), dialog: 1, pc_name: "Vel", npc: &info, generic: &mut generic, vfs: None, rng: &mut rng, deltas: Vec::new(), jumps: 0, areas: &crate::areas::NO_AREAS, pc_at: arcanum_formats::map::TileLoc { x: 0, y: 0 } };
        let Shown::Line(_, options) = say.show(&file, 1, &mut talk) else { panic!() };
        let texts: Vec<&str> = options.iter().map(|o| o.text.as_str()).collect();
        assert_eq!(texts, ["You stink.", "What news?"]);
        // The insult: reaction -10, still 21 or more, so on to line 1.
        let before = talk.reaction();
        say.deltas.clear();
        assert!(matches!(say.chosen(&file, 2, &mut talk, 0), Shown::Line(1, _)));
        assert_eq!(talk.reaction(), before - 10);
        talk.pc.reactions.insert("npc".into(), 25);
        say.deltas.clear();
        assert!(matches!(say.chosen(&file, 2, &mut talk, 0), Shown::End), "below 21: insulted back");
        let said = &say.deltas[0];
        assert!(matches!(said, LedgerDelta::FloatText { text, voice: Some(v), .. } if text == "Mind your tongue, Vel." && v == "sound/speech/01848/v4_m.mp3"), "{said:?}");
        // The story question: the answer, then one thanks that goes on to the response.
        say.deltas.clear();
        let Shown::Line(_, options) = say.chosen(&file, 3, &mut talk, 0) else { panic!() };
        assert_eq!(floats(&say.deltas)[0].0, "The war goes badly.");
        assert_eq!(options, [Offered { text: "Thanks.".into(), act: OptionAct::Respond(1) }]);
        // An I: NPC line insults by race (the override first).
        say.deltas.clear();
        say.show(&file, 10, &mut talk);
        assert_eq!(floats(&say.deltas)[0].0, "Mind your tongue, Vel.");
    }

    #[test]
    fn service_arguments_and_the_quest_table() {
        assert_eq!(number_list("9, 11"), [9, 11]);
        assert_eq!(number_list("1040-1043"), [1040, 1041, 1042, 1043]);
        assert_eq!(priced_list("$0,1040-1041"), (0, vec![1040, 1041]));
        assert_eq!(priced_list("($25, 1020-1021, 7)"), (25, vec![1020, 1021, 7]));
        assert_eq!(priced_list(" 9"), (0, vec![9]));
        let row = "1 50 301 302 303 304 305 307 308 314 315 316 317 318 319 319 401 -1 403 404 405 407 408";
        assert_eq!(quest_option(row, 0, 8, 50), Some(301), "high set");
        assert_eq!(quest_option(row, 2, 8, 10), Some(316), "low reaction: the middle set");
        assert_eq!(quest_option(row, 1, 4, 90), None, "dumb: the last set, -1 none");
        assert_eq!(quest_option(row, 9, 8, 50), Some(308), "states past 6 count as 6");
    }

    #[test]
    fn rumours_and_training_through_the_price_dialogue() {
        let file = DialogFile::parse(
            b"{1}{Well?}{}{}{}{}{}\n\
              {2}{R:$0, 1040-1041}{}{1}{}{1}{}\n\
              {3}{R:($30, 1050)}{}{1}{}{1}{}\n\
              {4}{T:9, 10}{}{1}{}{1}{}\n\
              {5}{Q:1000}{}{1}{}{1}{}\n\
              {6}{Another NPC line.}{}{}{}{}{}\n\
              {7}{About the tome.}{}{1}{}{1}{}",
        );
        let rules = RulesData::default();
        let mut sheet = CharacterSheet { gender: arcanum_rules::MALE, ..Default::default() };
        sheet.skills[9] = 1;
        let (mut inventory, mut pc, mut globals, mut attachment): (Inventory, PcScriptState, ScriptGlobals, Attachment) = Default::default();
        let mut talk = talk_with(&rules, &sheet, &mut inventory, &mut pc, &mut globals, &mut attachment);
        talk.globals.quelled.insert(1041);
        let mut generic = generic_with(&[
            ("mes/game_rd_npc_m2m.mes", "{20801}{Bates is a fraud.}{21001}{The mine is haunted.}"),
            ("mes/gd_cls_m2m.mes", "{1000}{That will be %d coins.}{2000}{You do not have enough coins.}{3000}{What shall I teach?}{4000}{You know that already.}{5000}{You are not ready.}{6000}{There, done.}{7000}{I know nothing.}"),
            ("mes/gd_cls_pc2m.mes", "{1000}{Thanks.}{2000}{Heard anything?}"),
            ("mes/gd_pc2m.mes", "{1}{Yes.}{100}{No.}{500}{Train me.}{600}{Go on.}{800}{Forget it.}{1000}{Thank you.}"),
            ("mes/skill.mes", "{9}{Haggle}{10}{Heal}"),
            ("Rules/gamequest.mes", "{1000}{1 50 7 7 7 7 7 7 7 7 7 7 7 7 7 7 7 7 7 7 7 7 7}"),
        ]);
        let info = npc(0);
        let mut rng = crate::HostRng::default();
        let mut say = Say { npc_net: NetId(7), dialog: 1, pc_name: "Vel", npc: &info, generic: &mut generic, vfs: None, rng: &mut rng, deltas: Vec::new(), jumps: 0, areas: &crate::areas::NO_AREAS, pc_at: arcanum_formats::map::TileLoc { x: 0, y: 0 } };
        let Shown::Line(_, options) = say.show(&file, 1, &mut talk) else { panic!() };
        let texts: Vec<&str> = options.iter().map(|o| o.text.as_str()).collect();
        assert_eq!(texts, ["Heard anything?", "Heard anything?", "Train me.", "About the tome."], "Q:1000 shows line 7");
        // A free rumour: 1041 is quelled, so 1040; the PC knows it now.
        say.deltas.clear();
        let Shown::Line(_, options) = say.chosen(&file, 2, &mut talk, 0) else { panic!() };
        assert_eq!(floats(&say.deltas)[0].0, "Bates is a fraud.");
        assert!(talk.pc.rumours.contains(&1040));
        assert_eq!(options, [Offered { text: "Thanks.".into(), act: OptionAct::Respond(1) }]);
        say.deltas.clear();
        let Shown::Line(_, _) = say.chosen(&file, 2, &mut talk, 0) else { panic!() };
        assert_eq!(floats(&say.deltas)[0].0, "I know nothing.", "none left");
        // A paid one: the price, scaled by reaction; too poor; then paid.
        say.deltas.clear();
        let Shown::Line(_, options) = say.chosen(&file, 3, &mut talk, 0) else { panic!() };
        let price = 30 * crate::barter::reaction_factor(talk.reaction()) / 100;
        assert_eq!(floats(&say.deltas)[0].0, format!("That will be {price} coins."));
        let OptionAct::Pay { price: asked, service, ret } = options[0].act else { panic!("{options:?}") };
        assert_eq!((asked, service, ret), (price, Service::Rumour(1050), 1));
        assert_eq!(options[1].act, OptionAct::Respond(1), "no");
        say.deltas.clear();
        say.pay(&file, asked, service, ret, &mut talk);
        assert_eq!(floats(&say.deltas)[0].0, "You do not have enough coins.");
        talk.change_gold(200);
        say.deltas.clear();
        say.pay(&file, asked, service, ret, &mut talk);
        assert_eq!(floats(&say.deltas)[0].0, "The mine is haunted.");
        assert_eq!(talk.gold(), 200 - price);
        // Training: Heal is below level 1, Haggle is ready.
        say.deltas.clear();
        let Shown::Line(_, options) = say.chosen(&file, 4, &mut talk, 0) else { panic!() };
        let names: Vec<&str> = options.iter().map(|o| o.text.as_str()).collect();
        assert_eq!(names, ["Haggle", "Heal", "Forget it."]);
        say.deltas.clear();
        say.train(&file, 10, 1, &mut talk);
        assert_eq!(floats(&say.deltas)[0].0, "You are not ready.");
        say.deltas.clear();
        let Shown::Line(_, options) = say.train(&file, 9, 1, &mut talk) else { panic!() };
        let OptionAct::Pay { price, service, ret } = options[0].act else { panic!() };
        assert_eq!(service, Service::Train(9));
        say.pay(&file, price, service, ret, &mut talk);
        assert_eq!(talk.sheet_changes.training, [(9, APPRENTICE)]);
        assert_eq!(floats(&say.deltas).last().unwrap().0, "There, done.");
    }

    #[test]
    fn newspaper_sellers_read_the_headline_and_sell_papers() {
        let file = DialogFile::parse(b"{1}{News?}{}{}{}{}{}\n{2}{P:}{}{1}{}{1}{}");
        let rules = RulesData::default();
        let sheet = CharacterSheet { gender: arcanum_rules::MALE, ..Default::default() };
        let (mut inventory, mut pc, mut globals, mut attachment): (Inventory, PcScriptState, ScriptGlobals, Attachment) = Default::default();
        let one_cell = |_: u32| Some(crate::ItemInfo { kind: arcanum_formats::object::TYPE_WRITTEN, inventory_art: 0, cells: (1, 1), weight: 1, magic_weight_adj: 0, complexity: 0, generic_flags: 0, weapon_flags: 0, armor_flags: 0, item_flags: 0, worth: 0 });
        let mut talk = talk_with(&rules, &sheet, &mut inventory, &mut pc, &mut globals, &mut attachment);
        talk.item_info = &one_cell;
        talk.change_gold(10);
        talk.globals.newspapers.queue = vec![-1, 1010, -1, -1, 5000];
        let mut generic = generic_with(&[
            ("mes/gd_npc_m2m.mes", "{4200}{What paper?}{4300}{Want it?}{4400}{No old papers.}{4500}{These old ones.}"),
            ("mes/gd_cls_m2m.mes", "{1000}{That will be %d coins.}{15000}{Here.}"),
            ("mes/gd_cls_pc2m.mes", "{1000}{Thanks.}"),
            ("mes/gd_pc2m.mes", "{1}{Yes.}{100}{No.}{800}{Forget it.}{1900}{A paper, please.}{2000}{Headline?}{2100}{Older ones?}{2200}{Today's.}"),
            ("mes/gamenewspaper.mes", "{1010}{490c BRIDGE FALLS}{5000}{490c GNOMES REJOICE}"),
        ]);
        let info = npc(0);
        let mut rng = crate::HostRng::default();
        let mut say = Say { npc_net: NetId(7), dialog: 1, pc_name: "Vel", npc: &info, generic: &mut generic, vfs: None, rng: &mut rng, deltas: Vec::new(), jumps: 0, areas: &crate::areas::NO_AREAS, pc_at: arcanum_formats::map::TileLoc { x: 0, y: 0 } };
        let Shown::Line(_, options) = say.show(&file, 1, &mut talk) else { panic!() };
        assert_eq!(options[0].text, "A paper, please.");
        say.deltas.clear();
        let Shown::Line(_, options) = say.chosen(&file, 2, &mut talk, 0) else { panic!() };
        assert_eq!(floats(&say.deltas)[0].0, "What paper?");
        assert_eq!(options.iter().map(|o| o.text.as_str()).collect::<Vec<_>>(), ["Today's.", "Headline?", "Older ones?", "Forget it."]);
        say.deltas.clear();
        let Shown::Line(_, options) = say.headline(&file, 1, &mut talk) else { panic!() };
        assert_eq!(floats(&say.deltas)[0].0, "GNOMES REJOICE. Want it?");
        assert_eq!(options[0].act, OptionAct::BuyPaper { paper: 5000, ret: 1 });
        say.deltas.clear();
        let Shown::Line(_, options) = say.old_papers(&file, 1, &mut talk) else { panic!() };
        assert_eq!(options[0], Offered { text: "BRIDGE FALLS.".into(), act: OptionAct::BuyPaper { paper: 1010, ret: 1 } });
        let Shown::Line(_, options) = say.ask_price(&file, NEWSPAPER_PRICE, Service::Newspaper(1010), 1, &mut talk) else { panic!() };
        let OptionAct::Pay { price, service, ret } = options[0].act else { panic!() };
        assert_eq!(price, 2, "the least price");
        say.pay(&file, price, service, ret, &mut talk);
        assert_eq!(talk.gold(), 8);
        let paper = talk.inventory.items.iter().find(|i| i.prototype == NEWSPAPER_PROTOTYPE).expect("bought");
        assert_eq!(paper.written, 1010);
    }

    #[test]
    fn healers_offer_what_they_can_and_heal_for_a_price() {
        let file = DialogFile::parse(b"{1}{Hurt?}{}{}{}{}{}\n{2}{H:}{}{1}{}{1}{}\n{3}{U:10}{}{1}{}{1}{}");
        let rules = RulesData::default();
        let sheet = CharacterSheet { gender: arcanum_rules::MALE, hp_damage: 60, poison: 70, ..Default::default() };
        let (mut inventory, mut pc, mut globals, mut attachment): (Inventory, PcScriptState, ScriptGlobals, Attachment) = Default::default();
        let mut talk = talk_with(&rules, &sheet, &mut inventory, &mut pc, &mut globals, &mut attachment);
        talk.change_gold(1000);
        let mut generic = generic_with(&[
            ("mes/gd_cls_m2m.mes", "{1000}{That will be %d coins.}{14000}{What healing?}{15000}{It is done.}"),
            ("mes/gd_cls_pc2m.mes", "{1000}{Thanks.}"),
            ("mes/gd_pc2m.mes", "{1}{Yes.}{100}{No.}{700}{I need healing.}{800}{Forget it.}{900}{[%s Skill]}{1000}{[%s Spell]}{1100}{[Magickal Healing]}{1200}{[Magickal Poison Healing]}"),
            ("mes/skill.mes", "{10}{Heal}"),
            ("mes/spell.mes", "{60}{Minor Healing}{61}{Halt Poison}{62}{Major Healing}{64}{Resurrect}"),
        ]);
        let mut info = npc(0);
        // Heal 3 points (level 12), Expert (rank 2); two white necromancy spells; a kit; aptitude 40.
        info.healer = Healer { college: 2, kit: true, aptitude: 40, ..Default::default() };
        info.healer.skills[HEAL_SKILL] = 3 | 2 << 6;
        let mut rng = crate::HostRng::default();
        let mut say = Say { npc_net: NetId(7), dialog: 1, pc_name: "Vel", npc: &info, generic: &mut generic, vfs: None, rng: &mut rng, deltas: Vec::new(), jumps: 0, areas: &crate::areas::NO_AREAS, pc_at: arcanum_formats::map::TileLoc { x: 0, y: 0 } };
        let Shown::Line(_, options) = say.show(&file, 1, &mut talk) else { panic!() };
        let texts: Vec<&str> = options.iter().map(|o| o.text.as_str()).collect();
        assert_eq!(texts, ["I need healing.", "[Heal Skill]"]);
        let Shown::Line(_, options) = say.chosen(&file, 2, &mut talk, 0) else { panic!() };
        assert_eq!(floats(&say.deltas).last().unwrap().0, "What healing?");
        let texts: Vec<&str> = options.iter().map(|o| o.text.as_str()).collect();
        assert_eq!(texts, ["[Heal Skill]", "[Magickal Healing]", "[Magickal Poison Healing]", "Forget it."], "no Resurrect below 5");
        assert_eq!(options[3].act, OptionAct::Respond(1));
        let Shown::Line(_, page) = say.heal_menu(&file, 1, 1, &mut talk) else { panic!() };
        let texts: Vec<&str> = page.iter().map(|o| o.text.as_str()).collect();
        assert_eq!(texts, ["[Minor Healing Spell]", "Forget it."], "Major Healing needs 3");
        // Minor Healing: 100 before the reaction, 5 + 25 x 40 / 100 = 15 hit points.
        let Shown::Line(_, ask) = say.heal(&file, Healing::Spell(MINOR_HEALING), 1, &mut talk) else { panic!() };
        let price = (SPELL_PRICE * crate::barter::reaction_factor(talk.reaction()) / 100).max(2);
        assert_eq!(floats(&say.deltas).last().unwrap().0, format!("That will be {price} coins."));
        let OptionAct::Pay { price: asked, service, ret } = ask[0].act else { panic!() };
        let Shown::Line(_, after) = say.pay(&file, asked, service, ret, &mut talk) else { panic!() };
        assert_eq!(talk.sheet_changes.heal, 15);
        assert_eq!(floats(&say.deltas).last().unwrap().0, "It is done.");
        assert_eq!(after, [Offered { text: "Thanks.".into(), act: OptionAct::Respond(1) }]);
        // The skill: (12 + 2) x (2 + 1) = 42 before the reaction; 1 to 18 hit points.
        let Shown::Line(_, ask) = say.chosen(&file, 3, &mut talk, 0) else { panic!() };
        let OptionAct::Pay { price, service, ret } = ask[0].act else { panic!() };
        assert_eq!(price, (42 * crate::barter::reaction_factor(talk.reaction()) / 100).max(2));
        say.pay(&file, price, service, ret, &mut talk);
        assert!((16..=33).contains(&talk.sheet_changes.heal), "{}", talk.sheet_changes.heal);
        // A follower heals for nothing; Halt Poison takes 50.
        talk.follow.mine = true;
        let gold = talk.gold();
        say.heal(&file, Healing::Spell(HALT_POISON), 1, &mut talk);
        assert_eq!((talk.gold(), talk.sheet_changes.cure_poison), (gold, 50));
        let mut healed = sheet.clone();
        talk.sheet_changes.apply(&mut healed);
        assert!(healed.hp_damage < 60 - 15 && healed.poison == 20, "{healed:?}");
        assert_eq!(spell_heal(MAJOR_HEALING, 100), 80);
        assert_eq!(spell_heal(MINOR_HEALING, -30), 5, "no aptitude: the least");
    }

    #[test]
    fn directions_list_unknown_areas_and_x_marks_them() {
        use crate::areas::{Area, AreaTable};
        let file = DialogFile::parse(
            b"{1}{Need anything?}{}{}{}{}{}\n\
              {2}{D:$0, 1, 3}{}{1}{}{1}{}\n\
              {3}{X:$0, 1}{}{1}{}{1}{}\n\
              {4}{D:$0, 2}{}{1}{}{1}{}",
        );
        let rules = RulesData::default();
        let sheet = CharacterSheet { gender: arcanum_rules::MALE, ..Default::default() };
        let (mut inventory, mut pc, mut globals, mut attachment): (Inventory, PcScriptState, ScriptGlobals, Attachment) = Default::default();
        pc.areas.insert(2);
        let mut talk = talk_with(&rules, &sheet, &mut inventory, &mut pc, &mut globals, &mut attachment);
        let area = |id, x, y, name: &str| Area { id, at: arcanum_formats::map::TileLoc { x, y }, name: name.into(), description: String::new(), radius: 5 };
        let areas = AreaTable::of(vec![area(1, 100, 1000, "Shrouded Hills"), area(2, 50, 50, "Crash Site"), area(3, 100, 100, "Black Root")]);
        let mut generic = generic_with(&[
            ("mes/gd_npc_m2m.mes", "{100}{Where to?}{500}{Which place?}{230}{East of here.}{730}{%d miles east.}"),
            ("mes/gd_pc2m.mes", "{1300}{Directions?}{1400}{Mark my map?}{800}{Never mind.}"),
            ("mes/gd_cls_pc2m.mes", "{1000}{Thanks.}"),
        ]);
        let info = npc(0);
        let mut rng = crate::HostRng::default();
        let mut say = Say { npc_net: NetId(7), dialog: 1, pc_name: "Vel", npc: &info, generic: &mut generic, vfs: None, rng: &mut rng, deltas: Vec::new(), jumps: 0, areas: &areas, pc_at: arcanum_formats::map::TileLoc { x: 100, y: 90 } };
        let Shown::Line(_, options) = say.show(&file, 1, &mut talk) else { panic!() };
        let texts: Vec<&str> = options.iter().map(|o| o.text.as_str()).collect();
        assert_eq!(texts, ["Directions?", "Mark my map?"], "D:2 left out: the Crash Site is known");
        say.deltas.clear();
        let Shown::Line(_, options) = say.chosen(&file, 2, &mut talk, 0) else { panic!() };
        assert_eq!(floats(&say.deltas)[0].0, "Where to?");
        let names: Vec<&str> = options.iter().map(|o| o.text.as_str()).collect();
        assert_eq!(names, ["Shrouded Hills", "Black Root", "Never mind."]);
        // Black Root is 10 tiles along y: screen down-right, rotation 3; near: row 200 + 30.
        say.deltas.clear();
        let OptionAct::Direct { area, price, mark, ret } = options[1].act else { panic!() };
        assert_eq!((area, price, mark, ret), (3, 0, false, 1));
        say.direct(&file, area, mark, ret, &mut talk);
        assert_eq!(floats(&say.deltas)[0].0, "East of here.");
        assert!(!talk.pc.areas.contains(&3), "D: does not mark");
        // X: marks Shrouded Hills, 910 tiles away: still under 2 × 3168, so row 600 + direction.
        let Shown::Line(_, options) = say.chosen(&file, 3, &mut talk, 0) else { panic!() };
        let OptionAct::Direct { area, mark, .. } = options[0].act else { panic!() };
        say.direct(&file, area, mark, 1, &mut talk);
        assert!(talk.pc.areas.contains(&1), "X: marks it known");
        assert!(talk.passes_test("ar 1") && !talk.passes_test("ar 3") && talk.passes_test("ar -3"));
        talk.pc_area = 2;
        assert!(talk.passes_test("ia 2") && talk.passes_test("ia -3"));
        talk.run_result("mm 9");
        assert!(talk.pc.areas.contains(&9));
    }

    #[test]
    fn alignment_aptitude_skills_training_and_items_by_name() {
        use arcanum_formats::object::{Object, Prototypes, Value};
        use arcanum_formats::test_util::{ObjectBuilder, prototype_id};
        let rules = RulesData::default();
        let mut sheet = CharacterSheet { gender: arcanum_rules::MALE, alignment: 30, ..Default::default() };
        sheet.skills[9] = 1;
        sheet.spells[2] = 3;
        let (mut inventory, mut pc, mut globals, mut attachment): (Inventory, PcScriptState, ScriptGlobals, Attachment) = Default::default();
        let mut prototypes = Prototypes::default();
        for (number, name) in [(6049, 2804), (6050, 5222)] {
            let mut o = Object::parse_instance(&ObjectBuilder::new(arcanum_formats::object::TYPE_WEAPON).field("name", Value::Int32(name)).build()).unwrap();
            o.id = prototype_id(number);
            prototypes.insert(o);
        }
        inventory.items.push(crate::CarriedItem { prototype: 6049, location: 0, quantity: 0, keys: vec![], written: 0, wear: Default::default() });
        let one_cell = |_: u32| Some(crate::ItemInfo { kind: arcanum_formats::object::TYPE_WEAPON, inventory_art: 0, cells: (1, 1), weight: 1, magic_weight_adj: 0, complexity: 0, generic_flags: 0, weapon_flags: 0, armor_flags: 0, item_flags: 0, worth: 0 });
        let mut talk = talk_with(&rules, &sheet, &mut inventory, &mut pc, &mut globals, &mut attachment);
        talk.prototypes = &prototypes;
        talk.item_info = &one_cell;
        talk.npc_inventory = Some(Inventory { items: vec![crate::CarriedItem { prototype: 6050, location: 3, quantity: 0, keys: vec![], written: 0, wear: Default::default() }], ..Default::default() });
        talk.npc_flags = 0x8;
        assert!(talk.passes_test("al 30, al -40, na 40") && !talk.passes_test("al 31") && !talk.passes_test("na -40"));
        let apt = rules.aptitude(&sheet);
        assert!(talk.passes_test(&format!("ma {apt}, ta {}", -apt)) && !talk.passes_test(&format!("ma {}", apt + 1)), "aptitude {apt}: magick up, tech down");
        assert!(talk.passes_test("sk 9 4, sk 9 -4, sc 2 3, sc 2 -3") && !talk.passes_test("sc 2 4"));
        assert!(talk.passes_test("wa 1, wt 0, fo 0, pa -7") && !talk.passes_test("fo 1"));
        assert!(talk.passes_test("in 2804, in -5222, ni 5222, ni -2804"), "items by name, the NPC's with -N");
        // Results: alignment like reaction, training one rank at a time, items move.
        talk.run_result("al +5");
        talk.run_result("al <20");
        assert_eq!(talk.sheet_changes.alignment, Some(20));
        talk.run_result("tr 9 2, tr 9 1");
        assert_eq!(talk.sheet_changes.training, [(9, 1)], "not two ranks at once; Apprentice at level 4 >= 1");
        assert!(talk.passes_test("tr 9 1"));
        talk.run_result("fp, in 2804, in -5222");
        assert_eq!(talk.sheet_changes.fate_points, 1);
        assert!(talk.passes_test("ni 2804, in 5222"), "the dagger went, the sword came");
        assert!(talk.npc_inventory_changed && talk.npc_has(2804) && !talk.npc_has(5222));
        let mut changed = sheet.clone();
        talk.sheet_changes.apply(&mut changed);
        assert_eq!((changed.alignment, changed.training[9], changed.fate_points), (20, 1, 1));
        // Experience is shared among the team, not applied here.
        assert_eq!(crate::experience_share(5000, 2), 2500);
        assert_eq!(crate::experience_share(1, 3), 1, "never 0");
        assert!(talk.unbuilt.is_empty(), "{:?}", talk.unbuilt);
    }
}
