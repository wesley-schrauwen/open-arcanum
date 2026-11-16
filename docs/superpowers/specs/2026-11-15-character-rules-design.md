# Character rules: skills, tech, magick

Status: design, approved approach 2026-11-15, not implemented.

## Intent

Build the original game's character progression once, as shared code: primary stats, basic and
tech skills with their training ranks, tech disciplines and degrees, schematics, spell colleges,
stat prerequisites and the magick/tech aptitude. Every screen that changes a character uses it:
character creation and the stats page after it (now), level-up and the in-game character screen
(later), trainers and found schematics (later, with the world).

Why now: it gets the project to the point where the player can finish "New Character" and be
taken into the world with a complete, valid character. The owner wants the full set at once,
because the screens share the same subroutines; building one screen at a time would mean
reworking those subroutines for each.

## Goals

- A new crate **`arcanum_rules`** with no Bevy dependency, holding the character state and every
  rule that changes it. The client and (later) the host call the same functions.
- Rules copied exactly from `Arcanum.exe` and the data, including odd behaviour.
- The character screen's Skills, Tech and Spells tabs (the original `Char_Maint` screen's
  top-right buttons and right-hand panel), shared by creation now and level-up later.
- The rules in `crates/arcanum_client/src/creation.rs` move into `arcanum_rules`.
- New Character → stats page → skills/tech/spells → loading, and the pre-made picker → loading,
  both ending with one `CharacterSheet` held by the host stand-in.
- Level-up rules (XP table, points per level, level cap, gaining a level) are included; the
  level-up screen is not.
- **On Create the host holds a `CharacterBlueprint`** (owner, 2026-11-15): everything needed to
  spawn the PC. It is the `CharacterSheet`, name, portrait, starting gold and starting items as
  item prototype numbers. Sources: `rules/backgrnd.mes` line 4 (money) and line 5 (item
  prototype numbers, space separated) for New Character; the PC prototype / `.mpc` inventory for
  pre-mades. Reverse engineer and copy any default items or gold every character gets, and
  whether the original auto-equips weapons and armour. Item objects, item prototype parsing,
  inventory and equipping are out of scope; the world turns the prototype numbers into objects
  later.

## Non-goals

- Networking. No `arcanum_sim`, `arcanum_net`, intents, transport or host in this work.
- Screens for trainers and for using found schematics, and crafting (they need the world, NPCs and
  items). Their rules are built; their UI is not.
- Casting spells, using skills in play, combat effects of skills, item failure from aptitude.
  Only the character-sheet side: what can be bought, what it costs, what it requires, and what it
  does to aptitude and derived values.
- Rebalancing. Balance and reworks of skills and magick come in a later stage.

## Anti-goals

- Rules duplicated in the client. Every check the UI makes is a call into `arcanum_rules`.
- "Fixing" the original while copying it. Behaviour that looks like a bug is copied and logged
  (see Faithfulness).
- Message or transport types designed now. The rules API is plain functions on plain data.
- Rules that read the VFS or Bevy resources at call time. Data is loaded once into a rules table
  and passed in.

## Constraints

- Bevy 0.20 client; workspace edition 2024. `arcanum_rules` may depend on `arcanum_formats` and
  `arcanum_vfs` (to load its tables), not on Bevy.
- Game data is not in the repo. Tests use synthetic `.mes` fixtures; real-data tests skip when
  `ARCANUM_DIR` is absent.
- English only. Text comes from the game's `.mes` files.

## What is known

Data files (all present in the archives):
- `mes/skill.mes`: 12 basic skills 0-11 and 4 tech skills 12-15; training ranks 17-19
  (Apprentice, Expert, Master); descriptions 20-35 name each skill's governing stat.
- `mes/tech.mes`: 8 disciplines 0-7 (Herbology .. Therapeutics), 8 degrees 8-15 (Layman ..
  Doctorate), descriptions per degree.
- `mes/spell.mes`: 80 spells 0-79, 16 colleges 500-515, descriptions 700+.
- `rules/SpellList.mes`: per-spell data; minimum IN per spell level (6, 9, 12, 15, 18).
- `rules/schematic.mes` (+ `mes/schematic_text.mes`, `art/item/item_schematic.mes`): schematics:
  name, text, drawing, ingredients, result, amount; degree-granted schematics numbered by
  discipline (1000, 1010, ...), found schematics separately.
- `mes/charedit.mes` 17-44: the original's refusal messages ("You must raise your Intelligence
  first", "cannot be lowered because ... prerequisite for a skill", "Minimum Level", ...).
- PC object fields: 66 base stats (28), 67 basic skills (12), 68 tech skills (4), 69 spell/tech
  (25), already parsed by `arcanum_formats::object`.

Not in the data, to be reverse engineered from `Arcanum.exe` (character screen code near
0x55a240-0x55e110, see NOTES.md "Character editor"): skill point costs and caps, how a skill's cap
depends on its governing stat, training rank requirements, the XP table, points per level and
the level cap, discipline degree costs and IN
requirements, spell costs, college order rules and level requirements, how each purchase moves
the magick/tech aptitude, and how skills feed derived values.

## Approaches

1. **Rules plus a client-to-host path now** (`arcanum_sim` + `arcanum_net` with a loopback
   transport, the client sends the accepted spend, the host re-runs the rules). Proves the reject
   path early. Rejected by the owner: building netcode before the world exists rests on large
   assumptions about it.
2. **Rules in the client, moved later.** Rejected: the rules must be shared with the host, and
   moving them later is a rewrite.
3. **Rules as a shared, Bevy-free crate (chosen).** One body of code with two callers: the host,
   which applies every change, and the client, which previews it. Until the host exists the client
   calls the operations in its place. Nothing guesses at the netcode.

## Owner's decisions

- Shared code, not per-screen code: one character system for creation, level-up, the in-game
  screen, trainers and schematics.
- The rules are shared utilities. The client applies a player's change at once as a preview; the
  host applies the same operation and is authoritative (accepts or rejects).
- **Characters exist only in the host's world.** Creation happens inside a session (single
  player: New Game starts the in-process host first). Every creation and level-up step is an
  operation the host applies; the host never receives a finished sheet to validate, so its ledger
  is correct by construction. Pre-made characters are built by the host from their PC
  prototypes. There is therefore no "validate a whole sheet" function and no action log on the
  sheet; the operations are the future intent set.
- No netcode in this work.
- **Faithfulness:** copy the original exactly for now. Balance and reworks come later.
- Accepted proposals:
  - **Accept-based spending.** As in the original ("below its last accepted level", charedit 19,
    23, 27), changes are a draft until the player accepts. Lowering is allowed only down to the
    last accepted value. Later, the accepted batch is what goes to the host.
  - **Training and found-schematic rules are built now,** their screens later.
- The rules live in a crate of their own, `arcanum_rules`, so the client does not depend on
  host code; `arcanum_sim` will depend on it.

## Design

### `arcanum_rules`

- **`RulesData`**: loaded once from the VFS (skill, tech, spell, schematic, background, effect,
  portrait tables). Replaces `CreationRules`.
- **`CharacterSheet`**: race, gender, age, level, XP, unspent points, base stats, bought HP and
  fatigue, skill points and training rank per skill, degree per discipline, known schematics,
  spells known (per college), aptitude, and the last accepted values needed for the lowering
  rule.
- **Constructors**: a starting sheet from race, gender and background; a sheet from a PC
  prototype (pre-mades; fields 66-69 via `arcanum_formats::object`).
- **Operations**: one function per player action (set race, gender, background, portrait, name;
  raise/lower a stat, skill, discipline or college; train a skill to a rank; learn a found
  schematic; gain a level; accept). Each takes
  `(&RulesData, &CharacterSheet, args)` and returns `Result<CharacterSheet, Refusal>`.
  `Refusal` carries the `charedit.mes` id the original shows (generalising today's
  `EditError::message_id`).
- **Queries**: costs, caps, prerequisites, derived stats, resistances, aptitude, and "why can't I"
  for disabled buttons, so the UI never computes rules itself.
- Each rule's doc comment names its `Arcanum.exe` address or data file.

### Client

- The client must not hold a sheet it treats as final. Screens keep a preview built with the
  rules; the sheet that counts belongs to the host. Until the host exists, one client-side stand-in
  (a single resource owning the "host" sheet and applying operations) plays that role, so
  `arcanum_sim` can later take its place without changing the screens.
- The picker and New Character both end with one `CharacterSheet` (replacing `ChosenCharacter`
  and `CreatedCharacter`).
- The stats page gains the Skills, Tech and Spells tabs, laid out from the original screen
  (buttons 527/595/663 at y 11, panel at 503,104; the rest from reverse engineering).
- Screens hold a `CharacterSheet` draft and call `arcanum_rules` for every check and change.
  Refusals show their message in the help box, as the stats page does now.
- Next from the editor accepts the sheet in the host stand-in and goes to loading.

### Faithfulness log

Behaviour that looks wrong is copied as is and logged in NOTES.md with its address, for the owner
to decide on later.

## Order of work (each its own PR)

1. Reverse engineer the rules listed under "What is known" into NOTES.md, via Sonnet sub-agents
   batched by topic, each number checked against the game data.
2. Data readers for the skill, tech, spell and schematic tables.
3. `arcanum_rules`: `RulesData`, `CharacterSheet`, operations, queries; move `creation.rs` in;
   unit tests for every rule.
4. The Skills, Tech and Spells tabs on the character screen.

## Left to the builder

- Exact type and function names, module layout inside `arcanum_rules`.
- Whether the rules data uses `MesFile` directly or its own parsed tables.
- How the tab panels lay out where the original's coordinates are not yet known.
- Test fixtures and how much of the real data the real-data tests cover.
