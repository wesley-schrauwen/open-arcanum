# Character Rules Foundation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Reverse engineer the original character rules, move today's creation rules into a
Bevy-free `arcanum_rules` crate as operations on a `CharacterSheet`, read the skill/tech/spell/
schematic tables, and end New Character and the pre-made picker with one `CharacterBlueprint`
held by a client-side host stand-in.

**Architecture:** `arcanum_rules` (depends on `arcanum_formats`, `arcanum_vfs`; no Bevy) owns
`RulesData` (tables loaded once), `CharacterSheet` (accepted values + a draft), operations
`RulesData::<op>(&self, &CharacterSheet, ..) -> Result<CharacterSheet, Refusal>` and a
`CharacterAction` enum with one `apply`. The client's `CharacterHost` resource is the only owner
of the sheet that counts; screens send actions to it and draw what it holds.

**Tech Stack:** Rust (edition 2024), Bevy 0.20 (client only), existing `MesFile`, `Vfs`, `Object`.

**Spec:** `docs/superpowers/specs/2026-11-15-character-rules-design.md`

**Scope:** This plan is part 1 of 2. The skill, tech discipline, spell and training *rules* and the
Skills/Tech/Spells tabs depend on the numbers Task 1 recovers; they get their own plan
(`2026-11-xx-character-rules-skills-tech-magick.md`) written from Task 1's NOTES.md section.

## Global Constraints

- `arcanum_rules` has no Bevy dependency. It may depend on `arcanum_formats` and `arcanum_vfs`.
- Rules copied exactly from `Arcanum.exe` and the data; behaviour that looks wrong is copied and
  logged in NOTES.md with its address. No rebalancing.
- No netcode: no `arcanum_sim`, `arcanum_net`, transport or message types.
- The client never computes a rule itself; every check is a call into `arcanum_rules`.
- Each rule's doc comment names its `Arcanum.exe` address or data file.
- Real-data tests skip cleanly without game data (follow `tests/vfs_assets.rs`'s pattern).
- Commit only after the owner has asked for commits in this session; work on a new branch off
  `feat/stat-screen` named `feat/character-rules`.
- Commit messages end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Review Focus

1. **Game data missing** (no `ARCANUM_DIR`): `RulesData::default()` must still give a usable sheet
   and host; no panic on empty portrait/background tables. Test in Task 3.
2. **Race or gender changed after points are spent:** the draft stays, the background and
   portrait reset, and a stat over the new race maximum does not panic or underflow. Test in
   Task 3. (Whether the original refunds is a Task 1 question; copy what it finds.)
3. **A pre-made whose prototype is missing or broken:** `PickPremade` of an unknown number is
   refused, and the picker still works. Test in Task 5.
4. **Next with no name:** the host refuses to finish with `MainMenu.mes` 506, as the screen does
   today. Test in Task 5.
5. **Background item lists with odd spacing or bad numbers** (`rules/backgrnd.mes` line 5):
   bad tokens are skipped, not fatal. Test in Task 5.

---

### Task 1: Reverse engineer the character rules into NOTES.md

**Files:**
- Modify: `NOTES.md` (new section "Character rules (skills, tech, magick)" after "Character
  editor")

**Interfaces:**
- Produces: the numbers and addresses part 2 and Tasks 3-5 rely on.

Use REA / `objdump` on `assets/Arcanum/Arcanum.exe`, dispatched as Sonnet sub-agents batched by
topic (the owner's preference, see memory `rea-via-sonnet-agents.md`), giving each the known
addresses from NOTES.md "Character editor" (0x55a240-0x55e110, +/- handlers 0x55a80a/0x55a997,
init 0x55e110). Check every number against the game data where the data has it.

- [ ] **Step 1: Skills batch.** Recover: skill point cost per step and how it changes with rank;
  skill cap and how it depends on the governing stat (and which stat governs each of the 16
  skills; `mes/skill.mes` 20-35 name them, confirm against the exe table); the training ranks
  (Apprentice/Expert/Master): requirements (skill level, stat) and effect; refusals 21-28.
- [ ] **Step 2: Tech batch.** Discipline degree cost; IN requirement per degree (charedit 26, 31);
  how a degree grants its schematic (`rules/schematic.mes` 2000 + 200·discipline + 10·(degree-1)
  for learned, 4000+ found); learning a found schematic (requirements); tech skills 12-15 vs
  disciplines.
- [ ] **Step 3: Magick batch.** Spell cost; college order (must spells in a college be bought in
  order); minimum IN per spell level and the WP limit (charedit 29, 39); minimum character
  level (charedit 30, 44); where `rules/SpellList.mes` stores each spell's data and how it is
  parsed.
- [ ] **Step 4: Aptitude and level batch.** How each skill/degree/spell purchase moves the
  magick/tech aptitude; the XP table (`Rules/xp_level.mes`?), points gained per level, level
  cap; whether stored base stats (`critter_stat_base_idx`) include race effects (check a
  pre-made: Lugard, prototype 16094, a dwarf); whether race or gender change refunds spent
  points; default starting gold and items every new character gets; whether the original
  auto-equips background items; where a pre-made's starting inventory lives.
- [ ] **Step 5: Write the NOTES.md section.** One table per batch: rule, value or formula,
  address, how verified. A "Copied oddities" list for anything that looks like a bug.
- [ ] **Step 6: Commit** `NOTES.md` with message "Notes: character rules from Arcanum.exe".

---

### Task 2: Create `arcanum_rules` and move `creation.rs` into it unchanged

**Files:**
- Create: `crates/arcanum_rules/Cargo.toml`, `crates/arcanum_rules/src/lib.rs`
- Move: `crates/arcanum_client/src/creation.rs` → `crates/arcanum_rules/src/creation.rs`
  (contents unchanged, tests included)
- Modify: `Cargo.toml` (workspace members), `crates/arcanum_client/Cargo.toml`,
  `crates/arcanum_client/src/lib.rs:10` (drop `pub mod creation;`), every `crate::creation::` /
  `arcanum_client::creation::` user (`create_character.rs`, `character_editor.rs`,
  `tests/vfs_assets.rs`, `tests/headless.rs`)

**Interfaces:**
- Produces: `arcanum_rules::creation::*` with today's names (`CreationRules`, `NewCharacter`,
  `EditError`, ...). Task 3 renames them.

- [ ] **Step 1:** `git mv` the file; `lib.rs` is `pub mod creation;`. `Cargo.toml`: package
  `arcanum_rules`, edition 2024, MIT, deps `arcanum_formats`, `arcanum_vfs` (path), dev-deps the
  same with `features = ["test-util"]`. Client depends on `arcanum_rules`.
- [ ] **Step 2:** Replace `crate::creation` with `arcanum_rules::creation` in the client and its
  tests.
- [ ] **Step 3: Verify.** Run: `cargo test --workspace`. Expected: same pass count as before the
  move (278 per NOTES.md, or whatever `git stash; cargo test --workspace` shows), 0 failures.
  Run: `cargo tree -p arcanum_rules | grep -c bevy`. Expected: `0`.
- [ ] **Step 4: Commit** "Move the creation rules into the arcanum_rules crate".

---

### Task 3: Reshape into `RulesData`, `CharacterSheet`, pure operations and `accept`

**Files:**
- Create: `crates/arcanum_rules/src/sheet.rs` (sheet, draft, `Refusal`, `Pool`),
  `crates/arcanum_rules/src/data.rs` (`RulesData`, `Background`, `Effect`, loading),
  `crates/arcanum_rules/src/ops.rs` (operations and queries on `RulesData`)
- Delete: `crates/arcanum_rules/src/creation.rs` (contents distributed; constants keep their
  names and doc comments: `RACE_COUNT`, `FEMALE_ALLOWED`, `FEMALE`, `MALE`, `PRIMARY_STATS`,
  `BASE_STAT`, `RESISTANCES`, `STARTING_POINTS`, `STAT_COST`, `MIN_STAT`, `MAX_NAME`,
  `XP_FOR_LEVEL_2`, `race_max`, `reaction_modifier`, `parse_effect`, `race_gender_code`,
  `Derived`)
- Modify: client `create_character.rs`, `character_editor.rs`, `tests/headless.rs`,
  `tests/vfs_assets.rs` (to the new API; behaviour unchanged)

**Interfaces:**
- Consumes: Task 2's crate.
- Produces:
  ```rust
  pub struct RulesData { pub backgrounds: Vec<Background>, pub portraits: Vec<(u32, String)>,
                         pub effects: Vec<(u32, Effect)> }            // was CreationRules
  impl RulesData { pub fn load(vfs: &Vfs) -> Result<Self, String>;
                   pub fn from_files(backgrounds: &MesFile, effects: &MesFile, portraits: &MesFile) -> Self; }
  pub struct Background { pub id: u32, pub description: u32, pub effect: Option<u32>,
                          pub codes: Vec<String>, pub npc: bool }    // NPC ones now kept, flagged
  pub enum Pool { HitPoints, Fatigue }
  pub struct Draft { pub stats: [i32; PRIMARY_STATS], pub hit_points: i32, pub fatigue: i32 }
  pub struct CharacterSheet {
      pub name: String, pub race: u8, pub gender: u8, pub age: i32, pub level: i32,
      pub background: u32,          // backgrnd.mes block id (was an index)
      pub portrait: Option<u32>,    // gameport.mes id (was an index)
      pub base: [i32; PRIMARY_STATS], // accepted base stats, without race/background effects
      pub hp_points: i32, pub fatigue_points: i32, // accepted points in each pool
      pub unspent: i32,             // accepted unspent character points
      pub draft: Draft,             // changes since the last accept
  }
  pub enum Refusal { AtMaximum, NoPoints, BelowAccepted, AtMinimum, NotAllowed, NoName, UnknownPremade }
  pub enum Message { Charedit(u32), MainMenu(u32) }
  impl Refusal { pub fn message(self) -> Option<Message>; } // 17,18,19,20 charedit; NoName MainMenu 506; others None
  // ops.rs, all on RulesData:
  pub fn new_sheet(&self) -> CharacterSheet;   // human male, background 0, first portrait, base [8;8], unspent 5
  pub fn step_race(&self, s: &CharacterSheet, delta: i32) -> CharacterSheet;
  pub fn toggle_gender(&self, s: &CharacterSheet) -> Result<CharacterSheet, Refusal>; // NotAllowed
  pub fn step_portrait(&self, s: &CharacterSheet, delta: i32) -> CharacterSheet;
  pub fn step_background(&self, s: &CharacterSheet, delta: i32) -> CharacterSheet;
  pub fn set_name(&self, s: &CharacterSheet, name: &str) -> CharacterSheet; // MAX_NAME, cp1252, no controls
  pub fn raise_stat(&self, s: &CharacterSheet, stat: usize) -> Result<CharacterSheet, Refusal>;
  pub fn lower_stat(&self, s: &CharacterSheet, stat: usize) -> Result<CharacterSheet, Refusal>;
  pub fn adjust_pool(&self, s: &CharacterSheet, pool: Pool, delta: i32) -> Result<CharacterSheet, Refusal>;
  pub fn accept(&self, s: &CharacterSheet) -> CharacterSheet; // folds draft into base/pools/unspent
  pub fn stats(&self, s: &CharacterSheet) -> [i32; PRIMARY_STATS]; // base + draft + race + background effects
  pub fn resistances(&self, s: &CharacterSheet) -> [i32; RESISTANCES];
  pub fn derived(&self, s: &CharacterSheet) -> Derived;
  pub fn unspent(&self, s: &CharacterSheet) -> i32; // accepted unspent minus draft spend
  pub fn background(&self, s: &CharacterSheet) -> Option<&Background>;
  pub fn portrait(&self, s: &CharacterSheet) -> Option<&str>;
  pub fn backgrounds_for(&self, race: u8, gender: u8) -> Vec<&Background>; // excludes npc
  pub fn portraits_for(&self, race: u8, gender: u8) -> Vec<&str>;
  ```
  Derived HP/fatigue use `hp_points + draft.hit_points` (and fatigue) in place of today's
  `hp_bought`.

- [ ] **Step 1: Port every existing test** in the moved `creation.rs` to the new API in
  `ops.rs`/`data.rs` test modules, keeping their values (e.g. dwarf stats `[9, 7, 9, 8, 8, 8, 8, 7]`,
  HP 30 → 34 after one pool step, `reaction_modifier(22) == 120`). Add:
  ```rust
  #[test] fn accept_folds_the_draft_and_blocks_lowering_below_it() {
      let r = rules(); let s = r.raise_stat(&r.new_sheet(), 0).unwrap();
      let s = r.accept(&s);
      assert_eq!((s.base[0], s.unspent, s.draft.stats[0]), (9, 4, 0));
      assert_eq!(r.lower_stat(&s, 0), Err(Refusal::BelowAccepted));
  }
  #[test] fn npc_backgrounds_are_kept_but_never_offered() {
      let r = rules();
      assert!(r.backgrounds.iter().any(|b| b.id == 39 && b.npc));
      assert!(!r.backgrounds_for(0, MALE).iter().any(|b| b.npc));
  }
  #[test] fn empty_rules_still_make_a_sheet() {   // Review Focus 1
      let r = RulesData::default(); let s = r.new_sheet();
      assert_eq!((s.portrait, r.portrait(&s), r.unspent(&s)), (None, None, STARTING_POINTS));
      let s = r.step_portrait(&s, 1); let s = r.step_background(&s, -1); let _ = r.step_race(&s, 1);
  }
  #[test] fn race_change_after_spending_keeps_the_draft_within_bounds() { // Review Focus 2
      let r = rules(); let mut s = r.new_sheet();
      for _ in 0..5 { s = r.raise_stat(&s, 0).unwrap(); }
      let s = r.step_race(&s, 1);
      assert_eq!((s.race, s.background, r.unspent(&s)), (1, 0, 0));
      assert!(r.stats(&s)[0] <= 30);
  }
  ```
- [ ] **Step 2:** Run `cargo test -p arcanum_rules`. Expected: FAIL to compile (new API missing).
- [ ] **Step 3: Implement** `sheet.rs`, `data.rs`, `ops.rs` with the signatures above;
  `lib.rs` re-exports them. Steps through backgrounds/portraits find the current id's position
  in `backgrounds_for`/`portraits_for` and wrap; `step_race`/`toggle_gender` reset `background`
  to 0 and `portrait` to the first of the new list, as today.
- [ ] **Step 4:** Move the client to the new API: `CharacterDraft(pub CharacterSheet)` (renamed
  in Task 5), `CreationData.rules: RulesData`, `texts(&self, c: &CharacterSheet)`; refusal text
  via `Refusal::message()` (`Charedit(id)` → `charedit`, `MainMenu(id)` → `menu`); name sync
  calls `set_name`.
- [ ] **Step 5: Verify.** Run: `cargo test --workspace`. Expected: all pass, count = Task 2's
  count + the 4 new tests.
- [ ] **Step 6: Commit** "Character rules as operations on a CharacterSheet".

---

### Task 4: Skill, tech, spell and schematic tables

**Files:**
- Create: `crates/arcanum_rules/src/tables.rs`
- Modify: `crates/arcanum_rules/src/data.rs` (`RulesData` gains the four tables; `load` reads
  their files), `crates/arcanum_client/tests/vfs_assets.rs` (real-data test)

**Interfaces:**
- Produces:
  ```rust
  pub const BASIC_SKILLS: usize = 12; pub const TECH_SKILLS: usize = 4;
  pub const DISCIPLINES: usize = 8; pub const DEGREES: usize = 8;
  pub const SPELLS: usize = 80; pub const COLLEGES: usize = 16; pub const SPELLS_PER_COLLEGE: usize = 5;
  pub struct Skill { pub name: String, pub description: String }        // skill.mes 0-15, 20-35
  pub struct SkillTable { pub skills: Vec<Skill>, pub ranks: [String; 3] } // ranks 17-19
  pub struct Discipline { pub name: String, pub description: String,
                          pub degree_descriptions: [String; DEGREES] }  // tech.mes 0-7, 16-23, 24+8d+k
  pub struct TechTable { pub disciplines: Vec<Discipline>, pub degrees: [String; DEGREES] } // 8-15
  pub struct Spell { pub name: String, pub description: String, pub college: usize } // spell.mes i, 700+i; college = i / 5
  pub struct SpellTable { pub spells: Vec<Spell>, pub colleges: Vec<String> }       // 500-515
  pub enum SchematicSource { Learned { discipline: usize, degree: usize }, Found { discipline: Option<usize> } }
  pub struct Schematic { pub id: u32, pub name: u32, pub text: u32, pub drawing: u32,
                         pub ingredients: [Vec<u32>; 2], pub result: Vec<u32>, pub amount: u32,
                         pub source: SchematicSource }
  pub fn parse_schematics(mes: &MesFile) -> Vec<Schematic>;
  // RulesData gains: pub skills: SkillTable, pub tech: TechTable, pub spells: SpellTable, pub schematics: Vec<Schematic>
  ```
  `rules/schematic.mes`: blocks at multiples of 10, lines 0-6 = name, text, drawing, item 1,
  item 2, result (each up to 3 space-separated prototype numbers), amount. 2000-3999 learned:
  discipline = (id - 2000) / 200, degree = (id % 200) / 10 + 1. 4000-5999 found: discipline =
  (id - 4000) / 200. Ids ≥ 6000: `Found { discipline: None }`. The text ids index
  `mes/schematic_text.mes`.

- [ ] **Step 1: Write failing tests** in `tables.rs` with synthetic `.mes`:
  `parse_schematics` on `{2000}{10}{2001}{11}{2002}{532}{2003}{10061}{2004}{10062}{2005}{10059}{2006}{5}{4210}{1}{4211}{2}{4212}{3}{4213}{1 2 3}{4214}{4}{4215}{5}{4216}{1}`
  gives two schematics: 2000 = `Learned { discipline: 0, degree: 1 }`, amount 5; 4210 =
  `Found { discipline: Some(1) }`, ingredients[0] `[1, 2, 3]`. A block with a non-numeric item
  line keeps the schematic and skips the bad token. Spell 7's college is 1.
- [ ] **Step 2:** Run `cargo test -p arcanum_rules tables`. Expected: FAIL (not defined).
- [ ] **Step 3: Implement** `tables.rs` and wire into `RulesData::load` (files:
  `mes/skill.mes`, `mes/tech.mes`, `mes/spell.mes`, `rules/schematic.mes`); add the paths to
  `resource_pack::paths::text` and `ALL_FILES`.
- [ ] **Step 4: Real-data test** in `tests/vfs_assets.rs` (skips without data): 16 skills with
  non-empty names, ranks `["Apprentice", "Expert", "Master"]`; 8 disciplines, degrees[0] ==
  "Layman", degrees[7] == "Doctorate"; 80 spells, 16 colleges, colleges[0] == "Conveyance";
  56 learned schematics, exactly 7 per discipline with degrees 1-7; 70 found.
- [ ] **Step 5:** Run `cargo test --workspace`. Expected: all pass.
- [ ] **Step 6: Commit** "Read the skill, tech, spell and schematic tables".

---

### Task 5: `CharacterBlueprint`, pre-made sheets and the `CharacterHost` stand-in

**Files:**
- Create: `crates/arcanum_rules/src/blueprint.rs`, `crates/arcanum_rules/src/action.rs`,
  `crates/arcanum_client/src/character_host.rs`
- Modify: `crates/arcanum_rules/src/data.rs` (`Background` gains `money`, `items`; `RulesData`
  gains `premades`), `crates/arcanum_client/src/lib.rs`, `create_character.rs`,
  `character_editor.rs`, `new_game.rs`, `main_menu.rs:338`, `tests/headless.rs`

**Interfaces:**
- Consumes: Tasks 3-4; Task 1's findings on base stats vs race effects, default gold/items and
  pre-made inventory.
- Produces:
  ```rust
  // Background gains: pub money: i32 (line 4), pub items: Vec<u32> (line 5, bad tokens skipped)
  pub struct Premade { pub prototype: u32, pub sheet: CharacterSheet, pub gold: i32, pub items: Vec<u32> }
  impl RulesData { pub fn premade_from_prototype(&self, prototype: u32, object: &Object) -> CharacterSheet; }
  // RulesData gains: pub premades: Vec<Premade>  (loaded from data/proto 16087-16098, as premade.rs does)
  pub struct CharacterBlueprint { pub sheet: CharacterSheet, pub gold: i32, pub items: Vec<u32>,
                                  pub prototype: Option<u32> }
  pub enum CharacterAction { StepRace(i32), ToggleGender, StepPortrait(i32), StepBackground(i32),
      SetName(String), RaiseStat(usize), LowerStat(usize), AdjustPool(Pool, i32),
      PickPremade(u32), Accept }
  impl RulesData {
      pub fn apply(&self, s: &CharacterSheet, a: &CharacterAction) -> Result<CharacterSheet, Refusal>;
      pub fn blueprint(&self, s: &CharacterSheet) -> Result<CharacterBlueprint, Refusal>; // NoName; accepts first
  }
  // client
  #[derive(Resource)] pub struct CharacterHost { sheet: CharacterSheet, premade: Option<u32>,
                                                 pub finished: Option<CharacterBlueprint> }
  impl CharacterHost { pub fn new(rules: &RulesData) -> Self;
                       pub fn sheet(&self) -> &CharacterSheet;
                       pub fn apply(&mut self, rules: &RulesData, a: CharacterAction) -> Result<(), Refusal>;
                       pub fn finish(&mut self, rules: &RulesData) -> Result<&CharacterBlueprint, Refusal>; }
  ```
  `CharacterDraft`, `CreatedCharacter` and `ChosenCharacter` are removed; screens read
  `host.sheet()` and send actions. A created character's blueprint: gold and items from its
  background (plus Task 1's defaults); a pre-made's: from its `Premade` entry, `prototype:
  Some(n)`. The name check (`MainMenu.mes` 506) moves from the screen into `blueprint`.

- [ ] **Step 1: Failing tests** in `arcanum_rules`:
  - `background_money_and_items_are_read`: `{3}{400}{4}{ 8054  x 8055 }` → money 400, items
    `[8054, 8055]` (Review Focus 5).
  - `apply_matches_the_operations`: `apply(s, RaiseStat(0))` == `raise_stat(s, 0)`; `Accept` ==
    `accept`.
  - `unknown_premade_is_refused`: `apply(s, PickPremade(99999))` == `Err(UnknownPremade)`
    (Review Focus 3).
  - `blueprint_needs_a_name`: `blueprint(&new_sheet())` == `Err(NoName)`; with a name and
    background 3 the blueprint has that background's money and items and an empty draft
    (Review Focus 4).
  - Real-data (in `tests/vfs_assets.rs`): `premade_from_prototype` for 16094 gives name "Lugard
    Bloodstone", race 1, background 46; `stats()` equals the values Task 1 recorded for him.
- [ ] **Step 2:** Run `cargo test -p arcanum_rules`. Expected: FAIL (not defined).
- [ ] **Step 3: Implement** `blueprint.rs`, `action.rs`, the `Background` fields and premade
  loading in `arcanum_rules`; `client::premade` keeps only display text and reads stats from
  `RulesData::premades`.
- [ ] **Step 4: Implement `CharacterHost`** and move the screens onto it: creation and editor
  actions → `host.apply`; editor Next → `host.finish` (refusal text in the help box) then
  loading; picker Next → `apply(PickPremade(p))`, `finish`, loading; `main_menu.rs:338` creates
  a fresh `CharacterHost::new(&rules)` on New Character.
- [ ] **Step 5: Update headless tests**: the picker test asserts `host.finished` has
  `prototype == Some(16094)`; the creation flow test asserts the finished blueprint's sheet
  instead of `CreatedCharacter`.
- [ ] **Step 6: Verify.** Run: `cargo test --workspace`. Expected: all pass. Run
  `DEBUG_ARCANUM_SCREEN=charactereditor DEBUG_ARCANUM_SCREENSHOT=/tmp/x.png cargo run -p arcanum_client`
  and check the stats page renders as before.
- [ ] **Step 7:** Update NOTES.md ("Layout and key files": `arcanum_rules`; "Character creation"/
  "Character editor": host stand-in) and commit "Character blueprint and host stand-in".
