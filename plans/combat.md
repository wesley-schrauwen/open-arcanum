# Combat: turn-based fights

Status: steps 1-18 built (`feat/combat-turns`, `feat/combat-npcs`, `feat/combat-ui`, `feat/combat-hooks`, `feat/combat-notice`, `feat/combat-allies`, `feat/combat-surrender`, `feat/combat-criticals`, `feat/repair`, `feat/combat-anims`, `feat/pc-death`, `feat/combat-events`, `feat/ammunition`, `feat/combat-stun`, `feat/sight`, `feat/combat-cursor`, `feat/knock-out`, `feat/npc-health`).

The owner's rules (`plans/multiplayer.md` 3, `plans/combat_design.md`): combat is turn-based
only; the host is authoritative over every result; any client may start a fight; the party
members within the engagement radius join it, the others move freely in real time; a member who
walks in joins at the bottom of the order next round. The companion directives and party stance
of `combat_design.md` come after the original's rules work.

## Findings (Arcanum.exe)

### The encounter (combat module 0x4b3xxx-0x4b8xxx)

- **Combat mode** per critter: `critter_flags` 0x40000 (0x4b3d50 tests, 0x4b3f80 toggles; enter
  0x4b3f20, leave 0x4b3f50; leaving refused while attacked: `intgame.mes` 24). A PC entering it
  in turn-based mode starts the fight (0x4b71e0).
- **Start** (0x4b71e0): every critter within 15 tiles (`0x5b57b8`) of the PC, sorted with PCs
  and their followers first, the rest in the order found (0x4b7eb0: no initiative). Critters
  not fighting are skipped as their turns come (0x4b7580).
- **Rounds** (0x4b73a0): the list is gathered and sorted again each round; with no active
  critter the fight ends (0x4b7330). Joining mid-fight: 0x4b7e00 (appends, sorts again).
- **Turns** (0x4b6e70): action points `max(5, speed)` (stat 0xb), none carried over; the UI's
  AP bar shows them (0x5fc1ec). Next turn 0x4b7690; the player's end turn 0x4b7c90 (points to
  0, next turn); a round's end waits 1000 ms (0x4b7740).
- **Costs** (0x557370 dispatch):
  - a step 2 points walking, 1 running (0x4b7ba0: path cost × 2, × 1 running);
  - an attack by the weapon's speed (0x4b7c30): up to 20, `max(1, 8 − speed / 3)`; 21-24, 2;
    25 or more, 1. Speed (0x465cf0): bare hands 10; a weapon's `weapon_speed_factor` plus its
    magick part, + 5 when trained in its skill;
  - using an item or scenery 1 (+ steps), a door or container 2 (+ steps), others 4
    (0x4b7a20 / 0x4b79a0 / 0x4b7aa0);
  - a PC with points left may act past them (0x4b7790); its turn then ends.

### The attack (0x4b3170)

- **Skill** (0x465d80): none, melee; `weapon_flags` 0x40, throwing; tech, range ≥ 3 and
  ammunition 1-3 (bullets, charges, fuel), firearms (tech 1); ammunition 0 (arrows), bow; else
  melee.
- **Chance** (0x4c62e0 / 0x4c69f0): bow and melee 5 × level + 25, throwing 7 × level + 25,
  dodge 2 × level, firearms (tech 0x4c69f0 index 1, checked) 5 × level + 25, − 20 against
  `spell_flags` 0x400000 of the target (corrected: field 0x14 at 0x4c6a34, firearms only;
  built, `feat/firearm-ward`, NPC targets);
  + 10 with an Intelligence of 20 or more (0x4c6369 / 0x4c6a62 → 0x4b0ee0 stat 4, effective,
  every skill: built, `feat/bright-skills`; not "effect 4"); the difficulty setting scales
  a PC's (0x404590: easy + half, hard − a quarter; no setting here). At most 95 only for skills
  of flag 0x800 in 0x5b6f64 (0x4c63b6 / 0x4c6aaf): dodge yes; bow, melee, throwing and
  firearms no (0x58f / 0x58d), and the roll (0x4c7259) caps nothing, so a master's 125 stays
  (`feat/uncapped-weapons`; before, all were capped).
- **Difficulty** (0x4c8430): −(the weapon's to-hit and its magick part), + 5 a point of
  Strength short of `weapon_min_strength`, range (bow and throwing below Expert: + 5 a tile past
  PE / 2; out of range impossible), light (`(255 − light) × 30 / 255`), cover, the target's state
  (−30 / −50 helpless, −30 unnoticed), the attacker's (blind + 30, crippled arms + 20 / + 50).
- **Roll:** 1-100 + difficulty ≤ chance hits. Critical hit 0x4c81a0 (a new 1-100 roll on a
  hit, at most `chance / D`: `D` 20 for skills with flag 0x400 in table 0x5b6f64 (bow, dodge,
  melee, throwing, firearms), else 2, checked at 0x4c81e4; + the weapon's field 0x82 scaled by
  0x461590; backstab and effect terms), critical miss 0x4c82e0
  (`(100 − chance) / 2` and the weapon's wear, at least 2), its effects 0x4b6410 (drop, break,
  wild hit by `weapon_crit_miss_chart`).
- **Dodge:** a hit is rolled again by the defender with Dodge; dodged: "Dodge!"
  (`combat.mes` 11). A critical dodge may counter (table 0x5b57a8: 0, 10, 50, 100 by training).
- **No armour class** term was found in the roll.
- **Damage** (0x4b6680, 0x465f70): each type (normal, poison, electrical, fire, fatigue)
  between the weapon's bounds plus its magick part; melee adds the damage bonus (stat 9) to
  normal and fatigue; bare hands: a PC 1-5 normal and fatigue (`max(g − 25, 1)` to `g + 5`, `g`
  the gloves' `armor_unarmed_bonus_damage`), a critter its `npc_damage_idx` pairs.
- **Resistance** (0x4b6860): type by table 0x5b5804 (damage, poison, electrical, fire,
  damage); fatigue three quarters; damage less its percent.
- **Critical multiplier** (0x4b5f40): `X` the weapon's field 0x83 (`weapon_magic_crit_hit_effect`,
  not for ranged weapons) + 10 on a called shot (dead in the original); × 3 on a roll up to `X + 10`, else × 2 on a
  second up to `X + 30`, else × 1.5 (checked: + 0xa, + 0x14, + 0x1e steps); an NPC's on a PC
  × 1.5.
- **Critical effects** (0x4b5f40 sets bits, 0x4b4390 applies them; not for `critter_flags` 0x4):
  - body part (0x4b65a0, checked): roll ≤ 70 torso, ≤ 85 legs, ≤ 95 arms, else head;
  - `E = X + 5`, + 10 with `weapon_crit_hit_chart` (0x81) 1 or 3 (no weapon or ranged: 1), + 10 on
    a bare head; saves: a roll ≤ CO + mod (0x45f060);
  - stunned (`combat.mes` 16, `critter_flags` 0x20), unconscious (17, fatigue + 10-20),
    knocked down (18), blinded (4, 0x80; head, chart not 1), crippled arm (5, 0x100, both 0x200;
    drops the weapon 19 / shield 20), crippled leg (6, 0x400), scarred (3, PCs, effect 0x32),
    weapon damaged / dropped (8 / 19), armour damaged (7), helmet knocked off (20);
  - items worn there lose 10 hit points (field 0x1d); at 0 broken, a broken one destroyed
    (0x43f1c0; floats 23 / 24);
  - a Master's Heal (0x4c7160 case 10) clears 0x80-0x400.
- **Critical miss** (0x4b6410): half of them have effects, on the attacker: the weapon destroyed
  (22), a spell weapon's spell on the fumbler (9), then 0x4b5f40's effects on it.
- **Applying** (0x4b4390): hit points `hp_damage` (field 0x1d); poison stat 0x18; fatigue. A
  critter's most hit points (0x43d5a0): `4 × hp_pts + hp_adj + WP + 2 × (level + ST) + 4`, as a
  PC's.
- **Death** (0x45d8d0: hit points < 1 → 0x45da20): script event 12 (dying); an NPC's killer
  gets a fifth of `npc_experience_worth`; each hit earned its share of the pool before
  (`npc_experience_pool`, damage percent × worth); the death art.

### NPCs (0x4a9650, 0x4ab460, 0x4aabe0)

- **Hostility** (0x4a9650): an attack makes the victim fight the attacker (its target in field
  0x11b, who hit it in 0x11c; setter 0x4aabe0 state 1).
  - Followers of both sides (0x4a9b80 → 0x4a9c00) fight the other side; one that likes the
    victim stays out (0x4adcc0) and likes its leader 5 less.
  - Witnesses (0x4a9e10): NPCs within a square of the radius (table 0x5b50c0 `{2, 8, 15, 1}`; a
    hit passes index 2) round the victim, through 0x4a9f10. Sides by 0x4ae3a0: itself, its
    leader, the same side (0x45d110: the same top leader or an equal non-zero `npc_faction`,
    0x45d080; with a PC, leading it or the PC's reputation with its faction, 0x4c21e0). An ally
    of the victim who notices it fights the attacker; an ally of the attacker who notices it
    fights the victim; one of neither side, idle, not `OCF_NO_FLEE`, who notices the attacker
    flees (0x4aabe0 state 2).
  - Noticing: sight 0x4af260 (`clamp(PE, 1, 10)` tiles, cut by stealth) or hearing 0x4af470.
  - The victim's reaction to a PC attacker: lowered to column 10 if above it (−10 for an
    indirect hit, flag 1); a leader hitting its follower sets column 9 and may make it rebel
    (0x4ad950). Hit and death witnesses within 10 tiles like the attacker's leader 20 less
    (0x4aee50, not built).
- **The AI tick** (0x4a84f0): 0x4a92d0 picks the state and target (its table 0x4a94a4: idle
  looks for targets 0x4ab460; fighting keeps or drops its target; fleeing 0x4aaf50; surrendered
  takes heart at 80 % hit points or more one tick in 500, 0x4a945a), then 0x4ac180 acts on it
  (table 0x4ac238: idle 0x4ac380, fighting 0x4ac6e0, fleeing 0x4ac620, surrendered 0x4ac660)
  unless a move is under way (`+0x14`). A fleeing NPC with no move left surrenders (0x4ac620 →
  0x4aabe0 state 3); a surrendered one goes back to idle when its target is gone, dead or down
  (0x4ac660), and only its target's attack makes it decide again (0x4aa620 case 3).
- **Keeping away** (0x4ac6e0 → 0x4ac7b0): a fighter whose row has column 15 above 1 and is
  nearer than that, one tick in 20, walks back to that distance. The rows' comments agree:
  "Generic Bowman" 4, "close combat invis will move back 5 tiles" 5.
- **States** (0x4aab00 reads them, 0x4aabe0 sets them; checked: 0x4aab00 is the getter): idle,
  fighting (`npc_flags` 0x1), fleeing (`critter_flags` 0x10, its target in 0xe6),
  surrendered (0x4000, the same target). Setting fighting raises event 13, leaving it 14. Fleeing and surrender by `rules/ai_params.mes` (41 rows of 17 numbers,
  row by field 0x11a): flee at the row's HP percent (0), against the target's strength (1-3).
- **Targets** (0x4ab460): critters within `clamp(PE, 1, 10)` tiles it sees or hears, nearest
  first, hostile by (0x4ae120): `npc_flags` 0x100 (kill on sight) without 0x800; a PC whose
  reaction is at or below the row's column 10; an alignment gap of at least column 11 (checked:
  the agent read 12; most rows hold 2001 there); one fighting its friend.
- **Noticing** (0x4aa0d0, heartbeat): critters within 10 tiles it sees or hears.
- **Script events:** 13 entering a fight, 14 leaving it (0x4aabe0); 12 dying; action 0x24
  makes the object attack each of a set (no ally pull, no reaction).

### Presentation

- **Combat button:** bottom bar (86, 457), art 470 `Combat_Button.ART`, hover `intgame.mes`
  1002; release toggles combat mode (0x54ebf0).
- **AP bar:** art 294 `AP_Bar.ART` at (222, 412) over the bottom bar; End Turn button art 297 at
  (320, 9) in it; 20 slots at x 22, 35, 48, 61, 74, 93, … 287 (table 0x5caa48), y 10: art 295
  green for points left, 296 orange for what a planned move costs, 315 red past them.
- **Animations:** 5 the combat stance (and sneaking), 6 run, 7 knocked down and the default
  death.
  - Attack (0x42b9c0 → 0x503f60, checked): 20 when the critter's body entry (table 0x5be980 by
    bits 24-26: human 1, dwarf 0, gnome/halfling 0, half-ogre 2, elf 1) or the monster's
    (0x5be994 by bits 23-27) is set, else 21; unique NPCs 20; 21 forced by `flags` 0x80 or while
    in animations 7-11; 14 for a wielded weapon with `weapon_flags` 0x40 (agent, not checked).
    The hit lands at the art's action frame (header + 0x1c, 0x432a8f); frame times scale by the
    weapon's speed (0x432c9e, 30-800 ms).
  - Death (0x4b5306, checked): more than 20 damage dies by the hit location (head 17, legs 19,
    else 18), else 7; 18 when the art for the chosen one is missing (0x42bce9).
  - No critter animation for being hit: an AnimFX by damage (0x433020, table 0x5de670); no dodge
    animation. Projectiles: `weapon_missile_aid` (0x4b39b0, goal 36).
  - Weapon code (bits 0-3): from the wielded weapon's `item_use_aid_fragment` (0x4650d0); 1
    unarmed.
- **Floats** (`combat.mes`, colours white, red, green, blue, yellow at 0x5b8e6c): "HT 7" (line
  1, white) for damage; "Dodge!" (11); "Critical hit" (12); "Damage +50% / +100% / +200%"
  (13-15); fatigue "FT" (10, red), poison (0, green); red over PCs, yellow over the player's
  followers.
- **Cursor:** art 353 `battlecur.art` in combat mode; a hovered critter gets `hoverattack.art`
  (555). A click on a critter attacks (0x54ed30 → 0x4445a0).

### `ai_params.mes` columns (agent's reading of the row's stack offsets; 0, 10, 11 checked)

| Col | Use |
|---|---|
| 0 | flee at or below this hit point percent (0x4ab2f0) |
| 1, 2 | flee when the target's group is col 1 larger, or its summed level col 2 higher (0x4ab2f0, 0x4ae0a0) |
| 3 | do not flee from a target whose hit point percent is below it |
| 4 | a new target farther than this is ignored while fleeing (0x4ab030) |
| 5-8 | reaction, alignment gaps and level for 0x4ad950 / 0x4adb50 |
| 9 | the reaction a leader's hit sets on its follower |
| 10 | hostile at or below this reaction; a hit lowers the reaction to it |
| 11 | hostile at this alignment gap |
| 12 | alignment ceiling of liking (0x4adcc0) |
| 13 | idle behaviour roll (0x4abc70) |
| 14 | a percent chance in combat (0x4a8940) |
| 15 | keep-away distance of a fighter (0x4ac6e0 / 0x4ac7b0) |
| 16 | unknown (0x4aeb10) |

## Steps

1. **Turns and the attack** (built, `feat/combat-turns`; `arcanum_sim::combat`).
   - `PlayerIntent::Attack` starts a fight (PCs within 15 tiles first, then fighting NPCs; the
     attacker leads) or strikes on the PC's turn, walking next to a melee target first as far
     as its points go; `PlayerIntent::EndTurn`. Moves in a fight: only on the PC's turn, 2 points
     a step, cut to what is left.
   - The attack as above without the light, range, cover, state and critical-effect terms;
     critical hits on a second roll at half the chance (corrected in step 8: a twentieth), the
     multiplier with no weapon bonus.
   - NPCs: an attacked NPC fights back (`Fighting`), joins the fight and keeps out of its idle
     wandering; its turns pass for now. Damage kept as `SectorDelta::npc_damage`; at 0 hit
     points it dies as action 0x3a kills, the experience shared.
   - Resistances: worn armour's, summed (builder's call: 0x43d6d0 not read).
   - Deltas: `Combat { order, turn, ap }` (empty order: over), `Attacked { attacker, target,
     outcome }`.
2. **NPC turns** (built, `feat/combat-npcs`).
   - A fighting NPC's turn: flee at its row's hit point percent (column 0; `critter_flags`
     0x8000000 never), walking up to 8 tiles away (builder's call) and dropping out; else attack
     its target, or the nearest standing PC of the fight, walking next to it first, until out of
     points. NPCs no longer fighting, or dead, pass.
   - PCs take hits on their sheets (hit points, poison, fatigue); a PC with no hit points takes no
     turns. A fight with no NPC fighting or no PC standing ends at once.
   - `HostRng` is xorshift64* now: plain xorshift's neighbouring outputs correlated under `%`
     (a 25 % roll missed 46 times in a row).
   - Not built: allies joining, surrender, ranged NPCs keeping their distance, fleeing weighed
     against the target.
3. **The client** (built, `feat/combat-ui`; client `combat_ui`).
   - The combat button toggles combat mode; in it, or while the player's PC fights, a click on
     an NPC sends `PlayerIntent::Attack`.
   - The AP bar (art 294 at (222, 412)) while the PC fights: green lights (295) for its points
     on its turn, End Turn (297) sends `PlayerIntent::EndTurn`; the bar takes its clicks.
   - Floats from `combat.mes` for each attack: "HT n", "FT n", "Dodge!", "Critical hit",
     "Damage +x%", "Critical miss"; white over NPCs, red over PCs (`FloatColour::Red`).
   - Checked by screenshot (`DEBUG_ARCANUM_PANEL=fight`): the bar with two lights left after
     the walk to the half-ogre guard.
   - Not built: the button's pressed look (the lights, cursors and animations came later).
4. **Hooks** (built, `feat/combat-hooks`).
   - Action 0x24: each NPC of the set fights the object (0x447d2f; the original skips the ally
     pull and reaction there too). Dialogue result `co`: the NPC fights the PC and the talk ends.
     Both go through `CombatStarts`: a fight starts round the target if none runs.
   - Script events on the NPC, the PC the triggerer: 13 entering a fight (attacked or set on),
     14 leaving it (fleeing), 12 dying (after the kill: builder's call, the original runs it
     first and a script may stop the death).
   - A PC at 0 hit points lies down as an unconscious one does until healed (builder's call,
     replaced by step 11's owner's call).
   - Real data: all 2399 scripts run to an end.
   - Hit / miss events: built in step 12.
   - Fate on blows (`feat/fate-attacks`, `combat::Fate`): after the roll and the dodge 0x4b3170
     asks 0x4f5270 (field 0xfe): a PC attacker's "critical success on next attack" (option 2,
     0x4b36a2) makes the blow a critical hit whatever came before (0x4b36b4: flags | 0x6), used
     up on that blow; else a PC target's "critical failure on next opponent attack" (3, 0x4b36e0)
     turns an NPC's blow into a critical miss (0x4b36f2-0x4b36f5 → 0x4b6410).
   - Fate on skills (`feat/fate-skills`, `take_fate`): 0x4c7160 asks it for PCs after the roll
     (0x4c72ef-0x4c737c): Gambling 6, Heal 7, Pick pocket 8, Pick locks 10, Disarm traps 11
     (Repair 9 too), each making the roll a critical success (0x4c7388). Built for Pick pocket,
     Pick locks and Disarm traps. Not built: Repair (here no roll: it restores by its chance),
     Gambling, Heal (no rolls kept).
5. **Noticing** (built, `feat/combat-notice`).
   - Every 15 ticks (builder's call: the original's noticing is in its target search 0x4ab460) each NPC not fighting,
     following, talking or dead looks for a PC within reach (`clamp(PE, 1, 10)` tiles, Perception
     cut by a prowler's value, or its hearing reach) it is hostile to (0x4ae120 as above); one
     found starts a fight (`CombatStarts`). Sight lines and facing are not counted.
   - The hover's to-hit panel in combat mode (0x554830): "To Hit:" and the weapon chance less the
     difficulty in the icon's place; the dodge and 0x461620 terms not counted.
   - Checked by screenshot: no fight starts at the house of the start tile `90330,84122` nor at
     the crash site.
6. **Others join** (built, `feat/combat-allies`; `combat::rally`).
   - Each attack (0x4a9650): NPCs within 15 tiles of the victim, not fighting yet: followers of
     either side, and NPCs allied to a side (`combat::allied`: a PC they follow, the same leader,
     the same non-zero `npc_faction`) who notice that side's critter (`notice_reach`), fight the
     other side and join the encounter (event 13); NPCs of neither side who notice the attacker,
     idle and not `OCF_NO_FLEE`, walk up to 8 tiles away (builder's call, as fleeing).
   - A PC attacker's victim likes it no more than its row's column 10.
   - The order: PCs, their fighting followers, the others (0x4b7eb0). Followers in a fight stop
     following and attack the nearest fighting NPC when their target is gone; a fight goes on
     while an NPC that follows no PC fights.
   - Distances are radii (owner's rule), not the original's square.
   - Bystanders (`feat/bystanders`): guards (`npc_social_class` 5, 0x4aa055) do not flee; the
     others that flee (state 2, 0x4aa0be) bark a `gd_npc` 3000 line (0x4aabe0 → 0x413ea0).
   - Finding: 0x4aee50 is not about attacks: it is the witnesses of a pick on an owned lock
     (callers 0x4c7ee4 / 0x4c7f1d in the skill use 0x4c7160, 0x4550ae, 0x4b4727 / 0x4b474c).
     Built in `feat/lock-witnesses` (`combat::catch_lock_pickers`): NPCs within 10 tiles of the
     lock, of its `portal_notify_npc` / `container_notify_npc` name or guards
     (`npc_social_class` 5), who see the PC: an opened lock, they fight it (0x4a9650); a failed
     pick, −20 reaction (0x4af00f → 0x4c0de0) and a fight at or under column 10 (builder's
     call for 0x4aaa60's threshold; checked in `feat/witness-bark`: dword 10 at 0x4af046).
     `feat/witness-bark`: a witness with a "catching thief" script (event 11, 0x4aefcd) is
     asked first, "skip" leaving it out; after a failed pick each barks a `gd_npc` 4000 line
     ("Stop that!", 0x414130 → 0x418780 lines 4000-4099). Not built: the other callers
     (bashing locks, 0x4b4390), the dialogue override's lines.
   - Followers refusing (`feat/follower-refusal`, `CombatWorld::follower_refusal`; 0x4a9c00 →
     0x4adcc0): a follower will not fight a foe that follows the same leader (code 4,
     0x4add66), whose alignment is at least its row's column 12 when it is good itself (stat
     0x13 above 0, code 1, 0x4adda4), of its own non-zero faction (2, 0x45d320) or allied to
     it (3, 0x45d110); bound followers (`spell_flags` 0x8000000) always fight. Refusing its
     attacking leader costs 5 reaction (0x4a9d26); the victim's followers just stay out.
   - Finding: the −10 "indirect" is the victim's, for indirect hits (0x4a9650); no witness
     reaction there.
   - The refusal bark: `gd_npc` 4900 (code 1), 5000 (2), 5100 (3), 4800 (4) (0x414370, table
     0x414478), before the −5.
   - A leader hitting its follower (`feat/leader-hits-follower`, `combat::leave_reason`;
     0x4a9650 when the victim's leader is the attacker, 0x4a9951): its reaction changes by its
     row's column 9 (0x4a996b), not to column 10; then 0x4ad950 (p5 1): 3 reaction at or under
     column 5 (0x4ada57), 1 / 2 the leader's alignment above / below its own by more than
     column 6 / 7 (0x4ada95 / 0x4adaac), 7 its level at least the leader's + column 8
     (0x4adae6); 0 when bound, the leader a Master of Persuasion (0x4ad9c9) or it joined by
     force (0x400). With a reason it leaves (0x45dfc0, LEFT) and says why (0x413a90: `gd_npc`
     1600 / 1700 / 1800 / 2900); else one time in three it complains (0x413df0: 2300).
   - Stray hits (owner's rule, `feat/stray-hits-keep-reaction`, `combat::Aim`,
     `leader_hit_reaction`): a blow that strikes a follower other than the one meant changes
     nothing in its reaction to the leader and sets no warning, as the original (0x4a9969: the
     indirect flag jumps past 0x4c0de0); the leave test still runs (0x4a9982). Stray hits are
     not made yet: every strike is `Aim::Meant`.
   - The rebuke (`feat/follower-rebuke`): the hit's loss goes through 0x4c0de0, which skips it
     for a leader of Charisma 20 or more (0x4c0e8e: 0x4b0ee0 with stat 7) and otherwise marks
     the follower to warn (0x10000 with 0x1000000, 0x4c0f06 / 0x4c0f18): at its next check
     (`follow::warning`, 0x4ac4c7 → 0x4adb50 → 0x414290) a follower within 20 of its limit says
     a `gd_npc` 5400 line.
   - Alignment warnings by script (`feat/script-alignment-warns`): action 0x89 moving a PC's
     alignment (0x4b0980 stat 0x13 → 0x4b0db8) marks its followers to warn, up good
     (0x2000000), down evil (0x4000000), as dialogue's change already did.
   - Prowling attackers (`feat/prowling-attackers`): the others drawn in by an attack notice a
     prowling PC by its stealth (0x4a9f10 → 0x4af260 / 0x4af470 with the prowl value), as
     noticing foes does.
   - A PC's reputation with a faction (`feat/faction-reputations`): an NPC also sides with a PC
     holding a reputation whose `rules/gamerep.mes` line lists the NPC's faction among its three
     (0x45d110 → 0x4c21e0: record +4 / +8 / +0xc; `ReputationEffects::lists_faction`), so it
     neither fights for its faction against that PC nor, as a follower, against it.
7. **Fleeing, surrender, keeping away** (built, `feat/combat-surrender`).
   - `should_flee` is 0x4ab2f0 in full: never from a target below column 3 percent; at or
     below column 0; else when the target's side is column 1 larger, or its levels column 2
     higher (a side: a PC and its followers, an NPC alone).
   - A fleeing NPC with nowhere to run (no path away within its points) surrenders
     (`combat::Surrendered`): out of the fight, still. It takes heart when whom it gave up to is
     gone, dead or down, or at 80 % hit points one tick in 500 (`recover_surrendered`); only
     that critter's attack makes it fight again. It notices no foes, does not wander and is not
     drawn into others' fights.
   - Keeping away: at the start of its turn, one turn in 20 (builder's call: the original rolls
     each AI tick), a fighter nearer than its column 15 distance walks back to it.
   - Attackers out of reach walk towards the target until in reach (ranged ones too).
   - The bark (`feat/surrender-bark`): fleeing or giving up (0x4aabe0 states 2 / 3, gated at
     0x4aad01-0x4aad2a) the NPC floats a `gd_npc` 3000-3099 line (0x413ea0 → 0x418780 with
     0xbb8 / 0xc1b; shown by the callback 0x5f8488 at 0x4aad65), for the PC it ran from.
   - Finding: script action 0x19 (table 0x44adbc, handler 0x44783a → 0x4c42f0) is not
     surrender: it adds a critter to an NPC's list field 0x104 with the time in 0x105.
   - Begging (`feat/surrendered-bark`): 0x4aa620's case 3 (field 0xda 0x4000,
     0x4aab51): attacked by whom it gave up to, or by 0x4ab0b0's pick, it fights (0x4ab2a0);
     by another, it floats a 3000-3099 line at that one when it can act (0x45d790 → 0x413ea0 at
     0x4aa759) and calls 0x434980: goal 0x19, `ag_flee` from that one (0x4349a6; table
     0x5a164c), when it can act (0x4348e0 → 0x45d790). Built (`feat/begging-flee`): it walks
     away as a fleeing fighter does, 8 tiles (builder's call: the goal's handler not read). Built for blows not thrown (0x4a9650) from a
     PC; builder's call: not when the blow kills it. Not built: 0x4ab0b0, the other
     callers 0x4a9c00 / 0x4a9f10 (each AI tick, which would bark at every foe in sight), action 0x19,
     ammunition (not found in the attack; built in step 13), the one-shot items of 0x4ac910.
8. **Critical hits** (built, `feat/combat-criticals`).
   - The critical chance is a twentieth of the skill chance plus `weapon_magic_crit_hit_chance`
     (unscaled: 0x461590 not read); the multiplier by the weapon's critical effect in three rolls;
     an NPC's on a PC × 1.5.
   - Injuries by body part: blinded, one or both arms crippled, legs crippled
     (`combat::critical_injuries`), all with the roll `E` (builder's call: the original's rolls
     for each differ a little). Kept in the PC's `CharacterSheet::injuries` and NPCs'
     `critter_flags` (`SectorDelta::npc_critter_flags`). Blinded + 30 and crippled arms + 20 /
     + 50 to the attacker's difficulty (0x4c8430). A Master healer's Heal cures them (builder's
     call: the rank test not traced). Floats "Blinded", "Crippled arm", "Crippled leg".
   - Not built: stunning, knocking out and down (their mechanics not traced), scars, items
     dropped or damaged by criticals, critical misses' effects.
   - Crippled legs (`feat/crippled-legs`): 0x4b0490 takes 5 off speed (stat 11) for
     `critter_flags` 0x400 (0x4b052a → 0x4b0533), before the effects (0x4b06f3): fewer action
     points a turn and a slower walk, PCs and NPCs. The only other test, 0x432dfc, skips an
     animation goal 0 after a move (purpose not found). Found nothing for to-hit, step cost or
     running. NPCs walked at a fixed 8 before (`feat/lame-pace`): now at their own speed
     (`npc_speed`), spawned with it and updated within 5 ticks of a change
     (`update_npc_paces`, `LedgerDelta::Speed`). Not built: the
     ±10 by global 0x458a80(0x4000000) and `spell_flags` 0x4000000 (0x4b04f4-0x4b0518).
9. **Item wear** (built, `feat/repair`; `arcanum_sim::repair`).
   - A critical hit's two item rolls (each up to the base + 10, 0x4b5f40 flags 0x100000 /
     0x4000) wear the armour where it landed (helmet, body, gloves, boots by part, 0x4b54b0) and
     the target's weapon by 10 hit points (0x4b4390); at 0, 0x43f1c0 (`feat/items-break`):
     an item (types 5-14, 0x43f4cd) not broken breaks: art bit 0x400 (0x5045c0 on fields 1 and
     0x5d; here `ItemWear::broken`), damage reset to 0 (0x43f522), off into the pack if worn
     (0x464c80) or to the ground without room (0x461a70), and its bust script, event 30, runs
     (triggerer the attacker, 0x4b4dc0; answer ignored). One already broken is destroyed
     (0x43f44a → 0x43cca0: its destroy script first; a 0 answer keeps it). A broken item cannot
     be worn or wielded (0x464d9f: reason 7, `mes/item.mes` 107); NPCs' picks skip it. The
     destroy waits a tick for its script (builder's call). Items with no hit points are left
     alone (builder's call). A broken item is painted red wherever its art is drawn
     (owner's call, `feat/broken-red`; the original swaps in a broken art, bit 0x400). Not built: 0x43f1c0's branches for other types (doors 2, containers 3,
     traps 17), the weapon's break sound (0x4f1050). Floats "Armor damaged", "Weapon damaged",
     "Armor broken", "Weapon broken". The wear is kept on the item, carried or dropped.
   - Repair (`feat/repair-broken`, 0x4c7160 case 0xc, training 0x4c67f0): nothing to do (505)
     without damage unless broken in a Master's hands (0x4c7c55-0x4c7c82); `max × chance / 100`
     restored, at least 1, none with no chance (501); below Master the most falls 10 / 5 / 1 %
     whether it worked or not (0x4c7daa); a Master loses none, and mends a broken item (art bit
     0x400 cleared, field 1 only) for 5 % (0x4c7d52-0x4c7d83), always 500. Below Master a broken
     item stays broken. Not built: a Master's 1 % on a critical failure with no chance
     (0x4c7d9f; 0x4c82e0 not decoded); the one-shot forced full repair (0x4f5270(src, 9)).
   - Firearms' chance corrected to 5 × level + 25 (tech table 0x4c6ad8 index 1).
   - Ordinary hits (`feat/armour-wear`): the damage array is normal, poison, electrical, fire,
     fatigue at +0x44 (entry [0x14] is fire, not item damage). A hit with fire damage (0x4b454f)
     or from a weapon of `weapon_flags` 0x100 (0x4b458d) sets the wear flag 0x100000; 0x4b4d5e →
     0x4b54b0 wears the armour at the hit's part by 10 (0 on an ordinary hit: body armour).
   - Finding: 0x4b5580 would wear the attacker's weapon (or hurt a bare hand) by the target's
     `material` (0x1e: 0 → 4, 1 → 3, 2 → 2, 3 → 1, 5 / 0xc → 5) less the weapon's
     `item_magic_tech_complexity` percent, gated at 0x4b55a7 by the attack's flag 0x10000,
     which nothing found sets (`[6] == 2`: unarmed or melee, 0x465d80). Not built.
   - Finding: no shield is worn: 0x4b23b0 is the slot-1004 item (the weapon), and flag 0x4000
     is the critical's weapon wear already built. Not built: critical misses wearing the
     attacker's weapon, worn items weakening.
10. **Animations** (built, `feat/combat-anims`; client `combat_ui`, `actors`).
   - On each attack the attacker faces its target and plays its attack once (20 / 21 by body;
     `Actor::action`, held at most 3 s).
   - The host picks the death by damage and hit location (`death_anim`, `death_frame_in`); the
     outcome says `killed`, and the client plays the death from its first frame when the dead
     NPC is spawned again.
   - Checked by screenshot: the fight runs at the crash site; the swing itself is for the eye
     (`plans/eye_checks.md`).
   - The hit's timing (`feat/hit-at-action-frame`): the attack goal charges its cost
     (0x432700), waits (0x42caa0) for the flag the frame step sets at frame `action_frame − 1`
     (0x42d051-0x42d05e; art header +0x1c through 0x5030b0), then resolves (0x42a630 →
     0x4b3bb0 → 0x4b3170); ranged blows the same (0x42ab90 → 0x4b3c00), with no projectile
     object on the way. Built on the client: a blow's floats and knock-down wait for the swing's
     action frame (`ArtFrames::action_frame`; at most 1.5 s). Builder's call: the host still
     resolves at once.
   - Swing speed (`feat/swing-speed`, `combat::swing_frame_millis`, `AttackOutcome::speed`):
     the attack goal's first state (0x432700, 0x43285e-0x4328c2) sets a frame's time to the
     art's `1000 / rate` less 10 ms a point of weapon speed over 10 (0x465cf0: 10 bare-handed,
     the weapon's + 5 trained), within 30-800 ms; the critter's speed stat plays no part. The
     client plays the swing at that pace.
   - Not built: hit effects.
   - The swing (`feat/attack-anim`, client `attack_anim`; corrected): 0x42b9c0 picks by the
     goal's target (0xd08), not the attacker: 14 for a thrown weapon (`weapon_flags` 0x40,
     0x42bacb, first; `AttackOutcome::thrown`); 21 at a target lying (animation 7-11) or of
     `flags` 0x80 (not known on clients); else 0x503f60 on the target's art: critters 20 (the
     body table is never indexed: 0x502710 gives 0 for them), monsters by table 0x5be994.
   - Arms in the art (`feat/weapon-art`, `combat::update_weapon_art`, `LedgerDelta::Armed`):
     0x465020 (on wielding, and with combat mode) sets art bits 0-3 to the wielded weapon's
     kind (`item_use_aid_fragment` bits 6-9; 1 bare-handed) and bit 19 for armour in slot 1005
     (a shield), only in combat mode (0x4b3d50, `critter_flags` 0x40000), else 0. Here as a
     critter enters or leaves a fight, and while it fights; one never in a fight keeps its
     placed art (builder's call).
11. **PC death** (built, `feat/pc-death`; owner's call: the original ends the game).
   - A PC whose hit points run out is dead (`CharacterSheet::dead`, `PcStatus::dead`): it lies
     where it fell, takes no turns or requests (moves, uses, rests, talks, picks), and no
     healing (scripts, dialogue, rest) raises it.
   - Resurrection: a healer of college 5 offers Resurrect while a PC of the party is dead;
     bought by another PC, every dead PC gets up with 15 % of its hit points, at least 1
     (owner's call: the original never resurrects a PC; first built with 1).
   - Owner's note, not built: moving a dead PC to the nearest inn (to be detailed).
   - The Resurrect spell (`feat/healing-spells`): any PC knowing it raises a dead PC or a dead
     NPC with 15 % of its hit points, at least 1 (`feat/resurrect-npc-15`).
   - Note: the list gives Resurrect `Dmg_Flags: Full | Resurrect`; the original's NPCs come back
     whole. 15 % for both is the owner's call (one rule for PCs and NPCs).
   - Effects cleared (owner's call, `feat/resurrect-effects`, `unconscious::resurrect`): a
     resurrected PC loses its effects of causes 3-9 (blessings, curses, items' and scripts'),
     as the original's reset 0x4a6010 does (0x4ea2e0 for each cause at 0x4a60b5-0x4a60cd);
     causes 0-2 stay. Poison is cleared too, PCs' and NPCs' (owner's call: the original keeps
     it, 0x4a609a). NPCs keep no effects here.
   - Not built: a dead PC's items and followers.
12. **Script events** (built, `feat/combat-events`; hook 0x441980 args: triggerer, attachee,
    extra, event, checked at 0x45da20).
   - Hit 7 / miss 8 on the attacker's weapon, a PC's or an NPC's (`feat/npc-weapon-events`:
     0x4b3770 and 0x4b2f60 test no critter type; an NPC's item is `ScriptObject::NpcCarried`,
     its prototype's scripts; triggerer the attacker; 0x4b37a3 / 0x4b3972), then
     critical hit 34 / miss 35 (0x4b37cd / 0x4b399c); critter hits 26 on the NPC attacker
     (triggerer the victim; 0x4b37fc). The weapon item is the attachee, the victim the extra;
     unarmed blows run none (0x4b3778-0x4b378b). The original runs 7 before the damage
     (0x4b37a3) and ignores every answer. Ranged blows run them at 0x4b2f90 / 0x4b3119 (34 / 35
     there only for weapon type 5: built) and a projectile's on arrival, on `projectile_parent_weapon`
     (0x4b2b7f / 0x4b2f06, from 0x4b2870). Leader killing 20 on each follower of the killing PC
     (triggerer the dead, 0x45db95). The original ignores their answers; here they run after
     the attack rather than before its damage.
   - Taking damage 23 on the NPC victim (0x4b446b, checked: a 0 answer jumps past the damage,
     0x4b447f): the damage waits a tick for the answer. The original runs it for any victim,
     PCs too (0x4b43ed-0x4b4446 tests no type), not for damage a script's run deals (guard
     0x5fc26c); PCs carry no scripts here, so for them it is nothing.
   - Will kill on sight 22 on a noticing NPC (0x4ae179, checked: a 0 answer is "not hostile",
     0x4ae183): the fight starts after its answer.
   - Start combat 15 (`feat/start-combat-event`, `EVENT_START_COMBAT`): 0x4a8570 (from the NPC
     think 0x4a84f0) runs it on every think of a leaderless NPC in a fight (0x4b3d50: `0xda`
     bit 0x40000), triggerer its target, the answer ignored (0x4a86ac); in real time end
     combat 16 first (0x4a866b, only when 0x5fc22c is 0). 13 / 14 are the edges (0x4aae29 /
     0x4aaf32). Here: 15 once at the start of each of its turns (builder's call: the
     original's think rate in turn-based play not traced); 16 never (turn-based only), not
     for followers, none without a player behind the target.
   - Destroy 2 for items destroyed in fights (`feat/items-break`): a weapon a critical miss blew
     up (0x4b4f48) and a broken item worn to nothing (0x43f44a) run it first, the item its
     triggerer and attachee (0x43cd0e); a 0 answer keeps it (0x43cd18). Will kill on sight 22
     has the one call site, 0x4ae179 in 0x4ae120 (callers 0x412fd0, 0x413130, 0x4ab460, not
     traced).
   - Resurrect 18 (`feat/resurrect-event`, `magick::resurrect_npcs`): 0x4b58c0 is the heal
     routine (action 0x4b, the Heal skill's case 10, spells at 0x452285). `Dmg_Flags` (names
     0x5b0d14, values 0x5b0d3c): Full 0x1, Resurrect 0x2, Reanimate 0x400002, Death 0x4,
     Stunned 0x40, Scars 0x1000, Scaled 0x800000, Weap_Dropped 0x2000, Crippling 0xe00. A heal
     reaching a dead critter does nothing without bit 2 (0x4b59e8); with it event 18 runs on
     the dead (triggerer the healer, 0x4b5a46), and a 0 answer, or `critter_flags` 0x1000000
     for a Resurrect (0x800000 for a Reanimate), refuses it: 0x45a540's eye candy (sound 0x25d,
     effect 0x283; not built). Here for Resurrect on a dead NPC (PCs carry no scripts; the
     owner's 15 % stays). Real data: 4 prototypes have 0x1000000, 4 have 0x800000. The raising (0x4b5a71,
     `feat/resurrect-raising`): fatigue damage 10 (0x4b5b28; the spell's heal gives none back:
     the `Full` branch takes over +0x54 only when above 0, 0x4b5c08); `Full` mends blinded and
     crippled (bits 0x80-0x400, 0x4b5c3d); a reaction r to a PC healer with 0 < r < 70 rises by
     70 − r (0x4b5b3d-0x4b5b9e; 0x4eaab0's scaling by the PC not decoded, so unscaled). Not
     built: stunned (0x20) cleared, the follower re-joined or dropped (0x45de90 / 0x45de50),
     the corpse's decay timer and marker removed (0x45ec80, 0x4b5e90), its AI timer reset
     (0x4ad6e0), effects of cause 7 removed five times (0x4ea2e0; NPCs keep no effects here).
   - Real data: prototypes attach 7 (10), 8 (7), 26 (9), 23 (3), 22 (1).
13. **Ammunition** (built, `feat/ammunition`).
   - Findings: ammunition is carried items of type 6 (`ammo_quantity` field 0x8e, `ammo_type`
     0x8f: 0 arrows, 1 bullets, 2 charges, 3 fuel); a weapon's `weapon_ammo_type` (0x7d, 10000
     none) and `weapon_ammo_consumption` (0x7e). The ranged attack animation checks the rounds
     (0x432cf0: count ≥ consumption, else no shot, no message); weapon choice skips weapons short
     of rounds (0x465170). The per-shot decrement was not found. Real data: 59 weapons use
     ammunition (consumption 1-6), 97 none.
   - Built: a shot needs and spends `weapon_ammo_consumption` rounds of its type (builder's call
     for the spending), emptied stacks gone. A PC short of rounds cannot shoot; an NPC fights
     bare-handed.
   - NPCs' weapons (`feat/npc-weapon-choice`, `combat::choose_weapon`): an NPC asked to
     (`feat/npc-rearm-flags`, `combat::rearm_npcs`: `npc_flags` 0x40 when an item enters its
     inventory, 0x46695a / 0x467c50; 0x80 when a shot lacks rounds, 0x4b3284 → 0x4adff0;
     cleared at 0x4a87f5 / 0x4a8840; 0x4b83e0 is the PC's, type 0xf at 0x4b840c) wields the
     best of its weapons with rounds enough (0x465170: score `chance × Σ mean damage / 100`, 0x4c62e0 / 0x465f70; ones reaching
     the target first, 0x465e30), or puts its weapon away for bare hands (0x464c50). Ties: the
     later item (the original's larger handle).
   - A shot short of rounds is no attack: the NPC re-picks and acts on; one that can change
     nothing fights bare-handed with the weapon in hand (builder's call).
   - Builder's call: only NPCs in a fight re-pick (the flag waits); 0x4a8570's gates (0x4233d0,
     field 0x13 & 0x102) are not decoded, and re-picking at peace would arm shopkeepers with
     the weapons sold to them. Followers never re-pick (0x4a8651).
   - Other slots (`feat/npc-repick-armour`, `combat::wear_best`): with 0x40 the NPC picks each
     worn slot 1000-1008 again (0x4654f0): the highest `item_worth` (at least 2, 0x461f20 at
     0x461f38) that fits, the worn one kept unless strictly beaten (0x46545c), one that no
     longer fits taken off (0x4651d4). The fit test's race comes from the art's body (builder's
     call).
   - Findings: a critical miss rolls its effects once (0x4b3770 skips the hit path at
     0x4b3778); the reset 0x4a6010 keeps poison (0x4a609a / 0x4a60ab). Correction: 0x49d100 is no
     resurrection: it restores a multiplayer character joining a game (sole caller 0x49cc70,
     the `.mpc` loader), keeping its poison over the reset (0x49d272 / 0x49d288).
   - The PC's auto-switch (0x4b83e0, after a shot short of rounds at 0x4b32a9 and a destroyed
     weapon at 0x4b5770) runs only with the "auto switch" option on (0x4b8330: key at 0x5b4cbc
     in the settings table 0x739e70, a missing key reads 0; multiplayer: per-player flag 0x400).
     Not built: the owner's Options decisions leave this setting out, so it stays off, as the
     original's default.
   - Not built: a message for the PC (the original shows none), the critical miss's ammunition
     loss (step 14), the skill gate 0x4c5eb0.
14. **Stuns, knock-downs, critical misses** (built, `feat/combat-stun`).
   - Findings: stunned (0x4b4390 → 0x4357b0: `critter_flags` 0x20, anim goal 57 with a count of
     `(20 - CO) / 2`, checked at 0x43581e) can do nothing (0x45d790 tests 0x60); in turn-based
     combat it loses each turn, 3 off the count (0x4b7580 → 0x427000), until under 1; in real
     time the goal counts down (0x426f60). Knocked down (goal 62, 0x435a90) costs no turn; it
     gets up on recovering fatigue (0x434de0); no timer found for a plain knock-down. Knocked
     out (0x100): fatigue damage to the most + 10-20 (NPCs only), with the knock-down.
   - Rolls (0x4b5f40): stun up to `E` with a CO save at − 5 (not when knocked down or stunned);
     knock down only for `critter_crit_hit_chart` 0, up to the base + 5, + 10 for weapon chart 1,
     + 10 on the legs.
   - Critical misses (0x4c82e0, checked: divisor 7 for combat skills at 0x4c8309, not 2):
     `(100 - chance) / 7` + the weapon's wear the same way + `weapon_magic_crit_miss_chance`, at
     least 2. Half of them (0x4b6410) have effects with `Y` the weapon's critical miss effect:
     destroyed (chart not 5, up to `Y + 1`); a shooter's ammunition all lost (up to `Y + 1`) or
     five shots' worth (up to `Y + 11`); then the attacker's own critical on itself.
   - Blowing up (`feat/weapon-explodes`, `combat::EXPLODING_CHARTS`): a weapon of critical miss
     chart 6, 7 or 8 (the Grenade Launcher; Bronwyck's Gun, the Flame Thrower, the Inferno
     Hammer; the Tesla Rod and Gun) rolls again up to `Y + 1` (0x4b650f-0x4b6531, flag
     0x10000): destroyed (0x4b4f28 adds 0x8000, float 22) with effect 0x37 (0x44c820 →
     0x44c7a0, the 0x5e2fb0 effects: not built). Real data: every weapon's `Y` is 0.
   - A weapon of object `flags` 0x400000 is never worn, destroyed or blown up (0x4b4e81 clears
     0x4000 / 0x8000 / 0x10000, so no float either; armour has no such test). Built
     (`feat/wear-immune`, `combat::OF_WEAR_IMMUNE`) from the prototype's flags: no prototype
     has it, and carried items keep no flags of their own here.
   - Built: all of that but knocking out and the real-time stun count (a stun ends with the
     fight, builder's call); knocked down plays the fall and the get-up on the client.
     Floats "Stunned", "Knocked down", "Weapon destroyed", "Ammo lost".
   - Weapon dropped (`feat/weapon-dropped`): a critical hit on an armed target rolls up to the
     base + 10 (0x4b6389, after the weapon's wear roll; never an NPC's hit on a PC) to knock its
     weapon to its tile (flag 0x2000, 0x4b4fdf → 0x461a90; a weapon broken by the same hit is
     gone). Float "Weapon dropped" (19).
   - Critical misses' items (`feat/fumble-items`, `AttackOutcome::fumble_items`): the
     attacker's critical on itself (0x4b6410 → 0x4b5f40 at 0x4b656c / 0x4b6593) rolls the item
     effects too, up to the base (field 0x83 through 0x4eab30) + 10: its weapon worn and knocked
     to its tile (not when destroyed: 0x8000 first), its armour worn where it struck. Floats over
     the attacker.
   - Not built: knocking out (built in step 17), 0x4b4390's second 0x4b5f40 call for the same
     critical miss (unverified), 0x4b65d0 (flags 0x20000 / 0x40000), the critical miss's spell weapons (0x4b65d0).
15. **Sight** (built, `feat/sight`).
   - Findings (0x4af260, returns 0 when seen; callers test `== 0`): 1000 for a fleeing, dead or
     stunned watcher (`critter_flags` 0x10a0) or an invisible target (unless the watcher has
     Perception 20); the facing cone 0x45eff0: the direction to the target within two
     rotations of the watcher's (5 of 8); reach `clamp(PE', 1, 10)` (0x4af240), `PE'` Perception
     less a prowler's stealth in percent (its Prowling value less 0x4c8430's difficulty: light
     `(255 - light) × 30 / 255` and its modifiers, blocked lines, the watcher's state); then the
     tile distance past the reach, + 1 when 0x4ade00 finds a blocker on the line (0x43fe00 with
     flags 0xc, not read). Hearing 0x4af470 subtracts a path cost along the line (0x4af640, the
     same walk with flags 0x20). 0x4aa0d0 is called from trap code (0x4bc2e0 / 0x4bc480), not on
     heartbeats: noticing there is a witness's.
   - Built: critters face their last step (`Facing`; before one, their art's rotation); NPCs
     notice foes (and allies notice in fights) by seeing (`combat::sees`: reach, cone, no tile
     on the line that blocks walking: builder's call for what blocks sight) or hearing.
   - Watcher state and invisibility (`feat/sight-state`, `combat::can_look`): a watcher with
     `critter_flags` 0x10a0 (stunned 0x20, blinded 0x80; checked at 0x4af287) or stunned sees
     nothing; a target with `flags` 0x200 or `spell_flags` 0x1 (0x4af2d6 / 0x4af2e7) is seen
     only with Perception 20 (0x4b0ee0: stat 5 at least 20) or `spell_flags` 0x200 (0x4af307).
     Hearing is unchanged. PCs are never invisible here (builder's call: nothing sets it yet).
   - The prowl value (`feat/prowl-opposed`, `prowl::Prowler`): Prowling is an opposed skill
     (0x5b6f64 flags 0x298, bit 0x10: the opponent's Perception), so 0x4c62e0 takes 0x4c6410:
     level − the watcher's PE, 100 at 10 or more, 0 at −10 or less, else table 0x5b7068 / 4;
     then + 10 at Intelligence 20; no cap. The level-by-five entry of 0x4c63dc never applies.
     Before, the value was level × 5 + 25 for every watcher.
   - Not built: light, the stealth difficulty terms.
   - What hides a target (`feat/sight-blockers`): 0x4af260 walks the line with the missile's
     flags (0x4ade00, 0xc at 0x4ade93) and a blocker adds 1 past the reach (0x4af45a), so it
     hides a target in reach; the line's cost is dropped (0x4af45e). Built: sight stops where a
     missile does (`MissileView`: walls on the crossed edge, closed portals, solid objects);
     the ground no longer hides anything. Critters on the line are left out (all shoot-through).
   - The hearing path cost (`feat/hearing-cost`, `combat::hears`, `sound_cost`): 0x4af470
     takes 0x4af640's cost off the reach (0x4af60c). 0x4af640 sums 0x43fe00 with flags 0x20
     over the line's steps (0x4af6d1): every wall on the crossed edge 2 whatever its piece
     (0x4400ba, 0x4404b8), a closed portal 1 (0x4401f1, 0x4405ef), entering ground that blocks
     8 (0x4d7180 at 0x440384 → 0x440390); objects and critters nothing (the cost block needs
     flag 8). Used by NPCs noticing and the scripts' can-hear. Builder's call: a diagonal step
     pays the cheaper of its two L-shaped routes. Not built: no line (0x4af6c0: 100), the sound
     table's other noises (0x5b50c0).
   - The listener (`feat/listener-state`, `combat::ears`): a stunned listener (`critter_flags`
     0x20 at 0x4af49b, or stunned by a blow) or a down one (0x45d800 at 0x4af4b1) hears nothing;
     one asleep (0x1000, 0x4af518) hears with half its Perception, halved before a prowler's
     cut (0x4af5a0-0x4af5ca: `r -= r × clamp(prowl − difficulty, 0, 100) / 100`, the magic
     0xae147ae1 a division by −100). Scripts' can-hear too, for NPC listeners.
16. **Planning and the cursor** (built, `feat/combat-cursor`; client `combat_ui`, `cursor`).
   - On the PC's turn the tile or NPC under the cursor gives the planned cost (2 a step; an NPC:
     the steps into reach and the attack's cost): the AP bar lights green for what stays, orange
     (296) for what the move spends, red (315) for what it lacks.
   - In combat mode or a fight the cursor is `battlecur.art` (353), over an NPC
     `hoverattack.art` (555), with the art's hotspot (builder's call).
   - Checked by screenshot (`DEBUG_ARCANUM_HOVER=700,600`, the fight at the crash site): 3
     orange and 9 red lights for a 12-point walk with 3 points left.
   - Finding: the original draws no planned path on the ground (0x56f430 draws only the AP
     bar's 20 slots into window 0x5caa18 through 0x51da80; no path or marker art found).
   - Owner's note, not built: an option to show the planned path on the ground (builder's
     call: off by default, as the original).
17. **NPC fatigue and knocking out** (built, `feat/knock-out`).
   - NPCs' fatigue is kept (`critter_fatigue_damage`, `SectorDelta::npc_fatigue`; most as
     0x45d670: `pts × 4 + adj + 4 + (level + CO) × 2 + WP`). Hits' fatigue damage lands on it
     (fists' 1-5, saps), and a critical knocks out (0x4b5f40 flag 0x100: after a stun, an NPC
     without `critter_flags` 0x4000000, another roll up to `E` and a CO save at − 5): fatigue to
     its most + 10-20 (0x4cef00(10, 20)). Float "Unconscious" (17).
   - Below 1 an NPC is down (0x45d800; never the undead, `critter_flags` 0x4 at 0x45d847,
     `feat/undead-tireless`): it lies on animation 7's last frame, loses its turns,
     counts for no side, notices nothing and does not wander. Its fatigue recovers by
     `(CO + 1) / 3` every 80 000 game ms (as PCs', 0x45e610); above 0 it gets up.
   - The client (`feat/npc-get-up`, `combat_ui::Lying`): a knocked-out NPC falls (7) when
     spawned again lying, and gets up (8) when spawned again standing; one raised from the dead
     gets up too (builder's call).
   - PCs out of fatigue (`feat/pc-out-turn`): 0x4b7c90 ends the current actor's turn (AP to 0,
     0x4b7cbd → 0x4b7690); fatigue damage that knocks a critter out calls it (0x45d5a0, at
     0x45d653), and turn starts skip critters that cannot act (0x4b7580 → 0x45d790: hit points
     or fatigue below 1, flags 0x60). Built: an unconscious PC's turn passes; an NPC with
     `critter_flags` 0x60 (stunned or paralyzed by its flags) loses its turns and notices no
     foe (`feat/paralyzed-turns`, `OCF_HELD`); NPCs pick
     unconscious PCs last (0x4ab460 adds 1000 to a downed target's score, does not exclude it).
   - Finding: the original has no "all PCs down" end of the fight in the functions read; here a
     fight still ends when no PC has hit points (unchanged). 0x4b6c90, 0x4af260 / 0x4af470's
     tests of downed targets not read.
18. **NPCs' health in the hover** (built, `feat/npc-health`).
   - Every 5 ticks the host compares each NPC's hit points and fatigue with what it last told,
     and sends `LedgerDelta::NpcHealth` on a change (at first sight only when not whole); the
     client keeps them (`combat_ui::NpcHealth`) and the hover's gauges show them as
     percentages (0x553f70).
   - Players who join later first get every NPC's health told so far that is not whole
     (`feat/npc-health-join`).
19. **Poison** (built, `feat/poison`; `arcanum_sim::poison`).
   - Findings: an attack's poison (damage type 1, at +0x48), cut by poison resistance (0x4b6860),
     adds to stat 0x18 of a PC or NPC alike (0x4b51a4). Time event 6 (0x4b1170, queued by
     0x4b1350): subtype 0 every 15 000 game ms (0x4b13ab) deals 1 hit point under 200, 2 under
     550, else 3 (0x4b11d1 / 0x4b11e2) as normal damage through 0x4b4390 (0x4b1203: it can
     kill) and does not lower the level; subtype 1 every 120 000 ms (0x4b12bb) lowers it by
     stat 0xd (poison recovery), to 0. Halt Poison subtracts its amount (script op 0x80).
   - Built: PCs' and NPCs' poison hurts and wears off on one clock (builder's call); NPCs keep
     it in `SectorDelta::npc_poison`; Halt Poison cures NPCs too.
   - Builder's calls: an NPC's recovery is its CN (at least 1); an NPC killed by poison dies on
     animation 7 (as 0x45da20 picks for 20 or less) without experience (no killer, as the
     original) or script events (the original runs 23 and 12 with no triggerer).
   - Immunity (`feat/poison-immunity`, `combat::shield_damage`): undead or mechanical critters
     (`critter_flags` 0x20000004, 0x4b441d) take no poison from attacks; else `spell_flags`
     0x10000 zeroes all damage (0x4b442e). The tick's hurt goes through 0x4b4390 (0x4b1203,
     its damage at +0x44): cut by damage resistance (worn armour), spared by `spell_flags`
     0x10000, floated "HT n" (0x4b5088, `LedgerDelta::Hurt`). 0x433020 is a particle effect
     on the target (not built); the struct's 0x80 flag changes nothing found.
   - Poison through resurrection: cleared (owner's call, step 11). 0x49d272 is the
     multiplayer join's restore, not resurrection.
20. **Barks in fights** (`feat/critical-praise` and on).
   - A PC's critical hit (0x4b374b → 0x4b6b90): a hit (flag 0x2, 0x4b6bee) and critical (0x4,
     0x4b6c0b) on the target meant, not on its own follower (0x4b6be4): half the time
     (0x4ae9fb) a follower picked at random (0x4aea3e) that can act (0x45d790) floats a
     `gd_npc` 2400 line (0x413f50 at 0x4aea84).
   - Stray remarks (`feat/stray-remark`, `combat::stray_remark`): a PC's hit (flag 0x2) on
     another than the critter meant (flag 0x8, 0x4b6c29), the one meant not its own follower
     (0x4b6be4), draws the same pick with mode 1: `gd_npc` 2500-2599 (0x413fce). Both remarks
     leave out followers joined by force (`npc_flags` 0x400: 0x45e3f0 counts them out at
     0x45e43b).
   - The bark veto (`feat/bark-veto`, `combat::bark_vetoed`): 0x413f50 and 0x413ea0 ask
     0x4ad800 (mode 1) first, and say nothing on a nonzero reason: the speaker destroyed or
     off (`flags` 0x3), stunned, paralyzed or mute (`critter_flags` 0x2060), asleep (0x1000),
     `spell_flags` 0x10000, dead or down, or another's follower (its leader, 0x4c1110, not the
     PC spoken to). Used for the flee, surrender and begging lines and the followers'
     remarks. Not built: condition 0x3a (0x459380), the dead's `spell_flags` 0x2000000.
   - Finding: 0x62ac6c (0x4ae9e0's gate) is the network layer's state (written by the TIG
     socket code 0x526xxx-0x52axxx): remarks are skipped with the net up and no session
     (bit 0 without bit 1). Nothing to build.
   - A PC's follower brought under 20 % of its hit points (0x4b50bb-0x4b50f1, before at least
     20 at 0x4b50d3, after under at 0x4b50e4) calls to its leader: `gd_npc` 5500 (0x4aeab0 →
     0x414490), once as it crosses, dying or not (`feat/follower-cry`).
21. **Stray throws** (`feat/stray-throws`, `combat::Aim`, `first_on_line`).
   - Findings: only the thrown path sets the stray flag 0x200 (0x4b3c00, `or 0x40240` at
     0x4b3d02, type 3); the redirect (0x4b33cf → 0x4ade00 at 0x4b33e9) puts the first object on
     the path before the target's tile in its place, with no chance roll, and the to-hit roll is
     made afresh against it (0x4b3405). Thrown attacks skip 0x4a9650 (0x4b3334, `ebx` 0): no
     one joins and no one's reaction moves, for the target meant or a stray one.
   - Built: a throw strikes the first critter on the straight line (builder's call: critters
     only, the line rather than 0x4201c0's path) as `Aim::Stray`; throws draw no one in and
     change no follower's reaction (with the owner's stray-hit rule); a stray throw draws no
     follower's remark (0x4b6bfc).
   - Blockers (`feat/throws-blocked`, then `feat/missile-blocking`): 0x4201c0 is a straight
     line (Bresenham, no pathfinding); 0x43fe00 (flags 0xc) stops a missile at walls on the
     crossed edge (door pieces as walking; a Passwalled wall only when not shoot-through),
     closed portals locked or not, living critters (0x440415: the dead let it by) and
     containers, scenery, projectiles and traps (0x440431, `NO_BLOCK` or not), each unless
     `OF_SHOOT_THROUGH` (0x20). Built as `blocking::missile_blocks` / `LoadedMap::missiles` /
     `MissileView`, the ground never tested (`first_on_line`: each step's edges, then the next
     tile's critters and objects, the target's tile left out).
   - Owner's rule (`feat/pc-shots-pass-followers`, `combat::missile_passes`): a PC's missile
     passes its own followers whatever their flags, and another PC in the way takes it (a stray
     hit); NPCs' missiles pass PCs, as the data.
   - Owner's call: arrows and bullets strike what is in their way too (the original redirects
     throws only, 0x4b3c00's flag 0x200).
   - Height is in the data: the shoot-through flag marks what a missile passes. Prototypes:
     small and mid stones, wooden and metal objects, small trees, plants, beds, machines pass;
     big trees, trees, dead trees, big stones, big metal objects, big machines, "something big"
     and passages stop it. All 510 critter prototypes (PCs' too) are shoot-through: in the
     data only an NPC placed without the flag stops a missile. Maps: 14 236 of 170 719 placed
     scenery stop missiles; Ruins of Szabo: 14 of 34 Big Stones (the others placed
     shoot-through), 4 Heavy Wood Doors, 458 walls; its stones, plants and bowmen pass.
   - Not built: cover (`PROVIDES_COVER` 0x4000 and not `SEE_THROUGH` 0x10 add cost, 0x44063f:
     its use in to-hit or sight not traced), the 2500 remark (flag 0x8).
   - Thrown weapons land (`feat/thrown-weapons-land`): every throw sets the projectile's
     `projectile_flags_combat` 0x40 (0x4b3d02, copied at 0x4b25fb), so on landing 0x4b2690
     moves `projectile_parent_weapon` (field 0x50, the whole thrown item: no stack split) to the
     projectile's tile and shows it (0x4b271d-0x4b2746); 0x1000 is ammunition's (0x4b25e7).
     Built: the thrown weapon leaves the thrower's hands and lands on the victim's tile, hit or
     miss, or on the tile where a wall or object stopped it. Builder's call: a PC's item goes
     the same way (the original removes only an NPC's at 0x4b3c4a; a PC's leaves through the
     interface, not traced).
24. **Light** (`feat/light-schemes`, `arcanum_sim::light`).
   - Findings (0x4c8923-0x4c8ae0): the term is skipped for an undead attacker (`critter_flags`
     0x4) and runs for skills of table bit 4 (bow, melee, throwing, Spot Trap, firearms, Pick
     Locks, Disarm Traps) or the prowling look (flag 0x4000); melee and Pick Locks are spared from
     Expert, Spot Trap, Disarm Traps and the prowling look from Apprentice. L is the light on the
     target (0x4dce10: its lit colour, field 0x145) or its tile (0x4d9240); for the prowling look,
     the prowler's own. `(255 − L) × 30 / 255`, then cut by the attacker's `critter_flags`
     0x400000 to a quarter, 0x200000 a third, 0x100000 half, 0x80000 three quarters; turned to
     30 − p by 0x45fb90 (field 0x101 = 0x4e or field 0xdb 0x800: a dark-seer). Hearing has none.
   - Ambient (0x4a8060 / 0x4a7eb0): `rules/Lighting Schemes.mes` names ten schemes; each file
     holds 24 hours of outdoor and indoor RGB, empty hours joined linearly. Tile art bit 8
     (0x5038c0) takes the indoor colour. Luminance: tables 0x739f18 / 30 / 20, `c / 255 × 76.5,
     150.45, 28.05`.
   - Built: the schemes read (all ten), the hour's ambient, the luminance and the dark's
     difficulty.
   - A tile's ambient (`feat/tile-ambient`, `LoadedMap::ambient_at`): the scheme of its sector
     (0x4d1916; 0 the default, 1; no map has `mapinfo.txt`), the hour's colour, outdoor when the
     ground art's outdoor flag (bit 8, 0x5038c0) is set (builder's call on which file triple
     0x4a7eb0 pairs with 0x602ea8). Maps on other schemes: K'na Tha (2, 9), Stillwater Giants
     Cave (2), The Pit (9), Tulla's Conveyance (10).
   - Sector lights (`feat/sector-lights`, `LoadedMap::light_at`, 0x4d89e0): on the ambient,
     each light not off (0x1) whose art frame covers the tile's centre adds its art pixel there
     (colour key: nothing); the art is the falloff, no radius. Coloured lights multiply the
     pixel; 0x8 / 0x10 take the outdoor / indoor ambient as the colour; channels add saturating
     at 255, a 0x2 light subtracts down to 0. Frame 0 of each light's art is decoded on sector
     load. On the start map a light lifts midnight's 74, 80, 98 to 118, 127, 156.
   - In to-hit (`feat/dark-to-hit`, `combat::dark_to_hit`): the light on the target's tile
     (builder's call: 0x4dce10's lit colour of the target itself not built), the scaling flags
     and dark sight as found: background 78 "Dark Sight" (`mes/gameback.mes` 1076) for PCs,
     `critter_flags2` 0x800 for NPCs. Noon outdoors costs nothing, the default night 20, the
     pitch dark 30.
   - The prowling look (`feat/dark-prowling`, `prowl::Prowler::look`): 0x4af260 subtracts
     0x4c8430 (skill 6, flag 0x4000) from the prowl value; its light term takes the prowler's
     own light and, for a watcher without dark sight, turns round (30 − p): `30 × L / 255`, so
     noon costs a prowler 30, the default night 10, the dark nothing. Apprentices and better are
     spared (0x4c8982). Hearing has no light term.
   - Skills (`feat/dark-skills`, `light::skill_dark`): Pick Locks (spared from Expert: the
     melee test at 0x4c8954 matches its index 2), Spot Trap and Disarm Traps (from Apprentice,
     0x4c8982) add the dark at the lock or trap to their roll. Builder's call: a trap spotted on
     use takes the light where the PC stands.
   - Dark-seeing watchers (`feat/dark-seeing-watchers`, `Prowler::for_watcher`): for one with
     dark sight (0x45fb90 at 0x4c8a9b: `critter_flags2` 0x800, a PC of background 78) the term
     is not turned round, so the dark gives a prowler away (30 in pitch dark) and light hides it.
   - The look's other terms (`feat/prowl-cover`, `Prowler::look_total`; 0x4c8430, prowling's
     table entry 0x298): the watcher's state as a target's (bit 0x8: paralyzed or down 50,
     knocked down or stunned 30, asleep 50 easier); the prowler blinded or lame + 30 each (bits
     0x80 / 0x200: 0x4c8b0e / 0x4c8b72); then the line (0x4ade00, 0x4c8bc6): a blocker hides it
     wholly (−100), else the shoot-through objects' cost comes off, and an open line costs 50 more
     below Expert (0x4c8bf4). In the open an untrained prowler is seen; behind a stone (20) it
     hides. Critters' cover on the line not counted (builder's call). No distance or armour term.
   - Borne light (`feat/carried-light`, `light::borne_light` / `borne_scale`): on each wield
     0x467e80 ORs the worn items' `item_flags` 0x200-0x1000 (`OIF_LIGHT_SMALL`-`_XLARGE`),
     shifted down by condition (0x461540: at most 25 % by three, 50 % two, 75 % one), and sets
     the largest as `critter_flags` 0x80000-0x400000 (`OCF_LIGHT_*`). It scales the dark's term
     for the bearer: a blow in the dark is easier with a lantern, and a prowler's lantern gives it
     away. In the data: Electric Light (extra large), Lantern and Helm of Light (large), Miner's
     Helmet (medium).
   - Objects' lights (`feat/object-lights`, `light::object_lights`, 0x4d9590): an object's
     `light_aid` (field 0x0e, −1 none) with `light_flags` (0x0d) and `light_color` (0x0f,
     `0xRRGGBB`, 0x4d84b0), and its four overlay lights (0x10-0x12), each at its tile and pixel
     offset (fields 2-4), lit as sector lights (`LoadedMap::object_lights`, rebuilt with the
     blocking). Builder's call: destroyed or off objects carry none. NPCs' own lights follow them
     (`feat/npc-lights-follow`: 0x43e9d9 → 0x4dd020 moves an object's lights with it), every 5
     ticks with the borne lights. Also: unloading a sector now forgets its missile, sound and light
     data too.
   - The bearer's light (`feat/borne-light`, `light::update_borne_lights`): after each wield
     0x467e80 gives the critter its own light (0x45fa70 → 0x43ebd0): art 0x504690(1) =
     0x90080000, coloured by size (0x45faa5-0x45fb54: extra large F4FFFF, large FFD6AC, medium
     EC8A55, small A7583A). Here every 5 ticks at each bearer's tile (PCs by what they wear, NPCs
     by flags or worn items), so a lantern lights its bearer and the ground round it.
   - The listen (`feat/prowl-hearing`, `Prowler::listen_total`): hearing's 0x4c8430 (flag
     0x2000) has no light and no line, but the watcher's state (as sight's), the prowler blind
     or lame + 30 each, and its worn armour's `armor_silent_move_adj` + magick part summed and
     taken off (0x4c8c03-0x4c8c91; halved from Apprentice, 0x4c8c74): heavy armour, a negative
     adjustment, makes it louder. The magick part unscaled (0x461590 not read).
   - Not built: animated light frames, 0x4dc210 (objects' lit colour over their footprint).
23. **Backstab** (`feat/backstab`, `combat::Stab`).
   - Eligible (0x45f9d0): a Backstab level (0x4c5eb0 skill 4), the target not having the
     attacker in view (0x45eff0 false), and a weapon whose wielded art (`item_use_aid_fragment`,
     an item art, bits 6-9 through 0x5044f0) is kind 2, or 3 / 4 / 7 from Expert (0x45fa41).
     Only melee blows at critters (0x4b22da, skill 2): flag 0x4000; 0x8000 besides when the
     target neither sees nor hears the attacker (0x4b2327, hearing with noise 0).
   - Damage (0x4b6680, normal damage only): + the level from behind, + 5 × it unnoticed
     (0x4b67bf-0x4b67d9), on the roll before resistance.
   - Unnoticed (`feat/backstab-unnoticed`): no armour term for one trained in Backstab
     (0x4c8743-0x4c875a); the critical chance + 2 × the level − the target's level (stat 0x11,
     0x4c8287-0x4c82a7), + 20 for a Master (0x4c82b6). The noise table 0x5b50c0 is 2, 8, 15, 1
     (`prowl::NOISES`): a quiet blow is heard within `clamp(PE' − 4, 1, 10) / 2` tiles.
   - The kinds from the data (`feat/backstab-weapons`): 2 daggers (Dagger, Stiletto, Railroad
     Spike), 3 swords, 4 axes, 7 great swords; 5 maces, 1 boomerangs, 6 pistols, 8 bows, 10
     rifles, 13 staves. So: a dagger always, a sword, axe or great sword from Expert.
   - Not built: 0x4a58d9's use of skill 4.
22. **To-hit difficulty** (0x4c8430, read in full; the roll `1-100 + difficulty <= chance` at
    0x4c7259-0x4c7261).
   - Terms for weapon skills: the weapon's to-hit bonus and 5 a Strength point short (built);
     the target helpless −50 (dead or down, `critter_flags` 0x40 / 0x1000) or −30 (stunned
     0x20, or it sees and hears no attacker: 0x4af260 / 0x4af470); a called-shot table
     (0x5b5798: dead in the original, not built: see below); armour; ranged only: past the weapon's range a sure miss, 5 a tile
     past half the attacker's Perception (not for a Master of the skill), the missile line's
     cost (0x4ade00) or a sure miss with a blocker; light `(255 − L) × 30 / 255` (melee
     Experts and better spared); injuries (built).
   - Called shots are dead code in the original (not built; #214 / #215 removed by
     `feat/no-called-shots`). A blow is aimed only if its hit location (A+0x40) is set before
     0x4b3170 (0x4b31ee: set → flag 0x1; unset → 0x4b65a0 rolls one). No caller sets it: a
     ranged blow passes 0 (0x42ac82 into 0x4b3c00); a melee blow passes the goal's +0x88
     (0x42a658 into 0x4b3bb0), zeroed at 0x44d46e and given no value by the attack goal's pusher
     (0x434b00). No UI strings for aiming. Untraced: goals restored from a save or the network.
   - Armour (`feat/armour-class`, `Fighter::armour_class`): half the target's armour class in
     the attacker's chance (0x4c875c-0x4c878b). Armour class (0x43d880): the `AC` field, worn
     armour's `armor_ac_adj` + `armor_magic_ac_adj` (0x465ba0, the magick part unscaled), and
     stat 10 (AC adjustment: DX − 10, as a PC's for NPCs, builder's call), at most 95.
   - Helpless targets (`feat/helpless-targets`, `Fighter::helpless`; 0x4c8609-0x4c86dc, one
     amount): 50 easier when down (out of fatigue or hit points, 0x45d800 at 0x4c8654); 30
     when stunned (0x20, 0x4c8684) or, not fighting yet (0x4b3d50), it neither sees nor hears
     the attacker (0x4af260 / 0x4af470 at 0x4c86b3 / 0x4c86c9). Builder's call: the blow that
     starts its fight counts as before it.
   - By state (`feat/helpless-flags`, `combat::helpless_by_state`, read in full
     0x4c8642-0x4c86d9): paralyzed (`critter_flags` 0x40) or down 50; knocked down (animation 7
     of the current art, 0x503e20 at 0x4c867b) or stunned (0x20) 30; asleep (0x1000) 50; then
     the unaware 30. For PCs and NPCs (types 0xf / 0x10 at 0x4c8621). Built from NPCs'
     `critter_flags`.
   - Knocked down (`feat/knocked-down`, `combat::KnockedDown`): the goal 62 handler (0x431b20)
     queues no get-up; goal 63 comes only from 0x434de0, which the fatigue recovery tick calls
     (0x45e610 at 0x45e75e) once fatigue is above 0 and the art lies on animation 7. Built: the
     host keeps it one recovery period (80 000 game ms, builder's call: the original's
     per-critter tick may come sooner); meanwhile 30 easier to hit (0x4c867b), not knocked down
     again (0x435a90's animation test) nor stunned (0x4b5f40); a critical miss knocks the
     attacker down the same way. Clients hold the lie (`Actor::knocked`, posture `Down`) until
     `LedgerDelta::GotUp` (`feat/hold-the-lie`), then play the get-up. While it lies its host
     art is animation 7 (`feat/lying-on-join`), so a player joining meanwhile spawns it lying,
     and `GotUp` stands it and plays the get-up.
   - Range and cover (`feat/range-and-cover`, `combat::missile_difficulty`,
     `blocking::missile_cost`): a ranged shot is 5 harder a tile past half the shooter's
     Perception (0x4c888e-0x4c88c1; a Master of the skill spared, 0x4c8883; NPCs never Masters
     here); the shoot-through objects on its line add 50 each when not `SEE_THROUGH` (0x10) and
     20 when seen through but `PROVIDES_COVER` (0x4000) (0x44063f; the target's tile left out).
     The game's Stone (flags 0x404830) gives cover, 20; plants none.
   - Critters, walls and portals (`feat/cover-critters`, `blocking::cover_cost`,
     `combat::line_critters_cost`): the same block (0x440671-0x440695) prices living critters
     on the line (types 0xf / 0x10 at 0x44064d, the dead skipped by 0x45d8d0), Passwalled walls
     (0x44028e) and closed portals (0x4401c3) that are shoot-through. Every critter prototype
     is 0x4030 (shoot-through, seen through, cover): 20 a critter. The game has one portal
     shoot-through but not seen through (50) and two walls that give cover (20). Builder's
     calls: a wall's or portal's cost counts on its own tile, as objects', the shooter's and
     target's left out; a PC's followers its shot passes still cost.
