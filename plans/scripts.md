# The script engine

The original's scripts decide what objects do when used, talked to, entered, killed and so on:
inn beds, quests, dialogue starts, traps, story state. This plan copies its engine
(Arcanum.exe 0x441980 hook, 0x4449b0 runner, 0x4451b0 conditions, 0x447220 actions) into
`arcanum_sim`, running on the host. Findings come from three Sonnet agents (2026-11-23), cross-checked where they disagreed.

## Multiplayer (owner's requirement)

The engine must work in multiplayer. The original runs every script on the host (and in single
player). A client runs only scripts 997 and 999 (end slides, credits) and treats every other
hook as "skip the default" (0x4449b0's gate on 0x62ac6c). Global state changes go through
helpers that apply them on the host and broadcast them (message 0x7c).

Here:
- **Host only.** Scripts run in `arcanum_sim`. Clients never run them; they see the results as
  deltas, as they do for doors and items.
- **State where it belongs:**
  - global variables, global flags and the story state go in the `WorldSave`;
  - an attachment's flags and counters go with the object (the save's sector changes for map
    objects, the PC's own record for a PC);
  - PC variables and flags go with each PC.
- **The triggerer** is the PC of the player who acted, never "the local player". Object set
  0x15 ("the local player", 0x40da50) needs a rule: the triggerer when there is one, else the
  party leader (builder's call, to confirm).
- **Effects addressed to players:**
  - float text and dialogue go to the triggering player, or to the players who can see it;
  - fades and teleports (actions 0x66/0x67) are skipped in multiplayer in the original, and
    here they move only the PCs involved.
- **Randomness** comes from the host's `HostRng`.
- **Big moves wait for everyone** (owner's decision, 2026-11-23):
  - What waits: a teleport, a map change (stairs up or down, entering a building such as a
    cathedral), anything instanced, or another sudden big movement. Every player sees a
    popup naming who caused it, and it happens only once all have ticked it.
  - Scripts: teleports (0x2c, 0x66) and their fades (0x67).
  - Not scripts: the map's jump points and stairs; see `plans/multiplayer.md`.
  - What does not wait: quests, the story state, reputation, gold, items, float text and
    dialogue.
  - For the runner: a run must be able to **pause** at a teleport and **resume** after the
    vote. Its state (line, locals, local objects, loop, pending script replacement) must be a
    value the host keeps across ticks, and in the save, not a stack frame.
  - Decided (2026-11-24, `plans/multiplayer.md`): the clock runs and all can act while waiting;
    anyone can decline, which cancels the move; leavers drop out; no timeout. Still open: combat
    during a vote.

## File format (`arcanum_formats::script`, done)

- **Finding a script:** script `n` is the `scr/*.scr` file whose name starts with `n`'s digits
  (`01729Bed_Use.scr`). Numbers below 1000 go in a separate table (0x4cece0). Duplicates are
  logged, and the first wins.
- **Header** (0x40 bytes):
  - 8 ignored bytes;
  - a 0x28-byte description;
  - flags at 0x30: bit 4 runs the script once; bits 8 and 0x10 concern dialogue (below);
  - the line count at 0x34;
  - the capacity at 0x38;
  - a stale pointer at 0x3c.
- **Lines:** each is 0x84 bytes: a condition block, a then block and an else block, 0x2c bytes
  each. A block is a type (`u32`), eight operand tags (`i8` at +4..+0xb) and eight operand
  values (`i32` from +0xc). Operands a type does not use are garbage. Operands are read in
  order (action 0xa: operand 0 := operand 1; 0xb: operand 0 := operand 1 + operand 2). An
  object operand's tag is an object set kind.
- **Checked:** all 2487 scripts in the archives parse exactly. Condition types run 0-52,
  action types 0-141.

## Running a script (0x441980, 0x4449b0)

- **The hook** `(triggerer, attachee, other, event)`:
  - it reads the attachee's `scripts_idx` (field 0x20) entry for the event: `{flags, counters, number}`;
  - with no script there it returns 1, "run the default";
  - with `event == 1` it first runs two checks (0x4bbfe0, 0x4bbe40; likely theft and witnesses, not decoded).
- **The runner** steps lines from a start line, at most 1000 steps. A line evaluates its
  condition and runs the then or else action, whose result decides what happens next:
  - -1: the next line; past the last line, end as -2;
  - -2 (action 1): end and skip the default; the hook returns 0;
  - -3 (action 2): end and run the default; the hook returns 1;
  - -4: end and run the default, without the run-once detach;
  - 0 or more: go to that line (action 3).
  - Hitting the step limit returns 0.
  - With flag 4, a -2 or -3 end clears the attachment's script number.
  - Actions 5 and 6 replace the attachment's script number after the run.
- **NPC dialogue (event 9) special cases** (0x4449b0), before running:
  - an NPC with flag 0x80000 in field 0x118 returns 1;
  - an NPC for which 0x45d8d0 is true (probably dead or unconscious) returns 1 unless script flag 8 is set;
  - one for which 0x4af210 is true returns 1 unless script flag 0x10 is set.
- **Conditions over object sets** count the members that pass. "Any" set kinds (3, 5, 7, 9,
  0xe, 0x10, 0x12, 0x14, 0x17) need one; the others need all, and at least one member.

## Operands

- **Scalar tags** (0x44ba70; verified against the jump table 0x44bb34):
  - 0: a counter byte of the attachment (0-3);
  - 1: global variable (2000);
  - 2: a variable local to the run (10);
  - 3: a number;
  - 4: global flag (3200);
  - 5: a PC variable (field 0x111);
  - 6: a PC flag (field 0x110);
  - for 5 and 6, the value's high 16 bits are an object set and the low 16 the index;
  - any other tag is a number.
  - The two agents disagreed on 1/4/5/6. The table settles it: 1 and 4 call 0x444c80 and
    0x444cf0 (globals), 5 and 6 call 0x444db0 and 0x444e30 (the PC's).
- **Object sets** (0x44b4e0):
  - 0 triggerer; 1 attachee (corrected 2026-11-24: 0x44b4e0 reads +0x8 and +0x10 of the run's
    objects, which 0x441980 fills with its first argument, the triggerer, and its second, the
    attachee whose `scripts_idx` it reads; the teleports' set 0 moving the PC confirms it);
  - 0xa the loop's object; 0xb a local object variable; 0xc the "other" object; 0x15 the
    local player;
  - 2/3 the triggerer's followers (0x4411c0 on +0x8); 0xd/0xe the followers and the triggerer;
  - 8/9 critters near the triggerer (to check against +0x8 as well); 0xf/0x10 scenery; 0x11/0x12 containers; 0x13/0x14
    portals; 0x16/0x17 items;
  - 4-7 not decoded.
- **Globals** (0x4446e0 init; saved by 0x4448d0 / loaded by 0x444930): 2000 variables, 100
  dwords of flags, the story state (0x5e2fa4, which only rises).

## Conditions (table 0x44714c, 53) and actions (table 0x44adbc, 141)

Decoded so far, with confidence; the rest are named by the helper they call.
- **Conditions:**
  - 0 always; 1 daytime;
  - 2 has gold ≥ V; 3 local flag; 4 V == V; 5 V ≤ V;
  - 6/7 quest state (per object / global); 8/9 has curse / blessing; 0xa reaction;
  - 0x11 has an item of a prototype; 0x12 leader is; 0x16 dead or down; 0x17 hit point ratio;
  - 0x20 at a location; 0x21 has reputation; 0x22 distance ≤ V;
  - 0x2b global flag set;
  - others named by their helpers.
- **Actions:**
  - control: 1 end and skip the default; 2 end and run the default; 3 go to a line;
    4 dialogue; 5 remove this script; 6 change it; 7 call a script;
  - flags and arithmetic: 8/9 set/clear a local flag; 0xa assign; 0xb-0xe add, subtract,
    multiply, divide;
  - quest state and loops: 0x10/0x11 set quest state; 0x12-0x14 loop over a set (for, end, break);
  - followers: 0x15/0x16 add/remove a follower;
  - curses and blessings: 0x19-0x1c add/remove;
  - reaction and stats: 0x1d-0x1f reaction; 0x21 a stat;
  - gold and items: 0x23 give/take gold; 0x25 random; 0x29 move an item;
  - story state and travel: 0x2a/0x2b story state; 0x2c teleport; 0x36/0x37 timed events;
  - float text: 0x40/0x41/0x72; 0x4c damage;
  - attachments and global flags: 0x63 attach a script; 0x64/0x65 set/clear a global flag;
  - fades: 0x66/0x67 fade and teleport.

## Steps (one PR each)

1. **The reader** (`arcanum_formats::script`, `ScriptIndex`) and this plan. Done.
2. **The runner on the host** (`arcanum_sim::script`). Done.
   - `run_hook` runs an `Attachment` for an `Invocation` (attachee, triggerer, other, event,
     start line) through a `ScriptContext` the host provides, and says whether the default
     runs.
   - Built:
     - conditions 0, 1, 3, 4, 5 and 0x2b;
     - actions 1-3, 5, 6, 8, 9, 0xa-0xf, 0x12-0x14, 0x25, 0x2a, 0x2b, 0x64 and 0x65;
     - object sets 0, 1, 0xa, 0xb and 0xc;
     - all scalar operands.
   - Global state is saved in the `WorldSave` (`scripts`), a PC's in its `PlayerSave`
     (`script_state`).
   - `ScriptLibrary` indexes `scr/` and parses on first use.
   - Loop break follows 0x44bc60: it goes past the first line from the loop's start whose
     condition is 0 and whose then action is 0x13. Random is inclusive (0x4cef00).
   - **Builder's call:** a condition, action or object set not built yet stops the script
     and runs the default, and the runner reports it, rather than half-running it.
   - **Real data:** from their first line, 459 of the 2399 scripts run to an end. The rest
     stop at, most often:
     - action 4, dialogue (262 scripts);
     - action 0x2c, teleport (261);
     - condition 6, quest state (139);
     - actions 0x22 and 0x51 (105 each);
     - action 0x18 (89);
     - condition 0x14 (80);
     - object set 8, critters near (67).
3. **Use on doors and containers** (`arcanum_sim::script_host`). Done.
   - Attachments are read from `scripts_idx`, the object's own or its prototype's. It is a
     sparse array: bit `e` of the bitset marks event `e`, with one 12-byte `{flags, counters,
     number}` entry per set bit, in order.
   - A changed attachment is kept in the sector's changes (`object_scripts`, `mobile_scripts`)
     and saved.
   - `ScriptHost::hook` runs the script with the acting player's PC as the triggerer. Doors
     (`use_portals`) and containers (`pick_up_items`) run it before their default.
   - **Traps** (scripts 30000-30007) are sprung by 0x4bcb70 before any line, and the use does
     not happen. Built in 4l.
   - **Start map** (around the start): 20 containers and 1 piece of scenery carry use
     scripts. Most are traps, plus 1180 ("Grave_Container_Use") and others not built yet,
     which run the default. 1621 ("Search the bed") keeps its bed shut until global flag 2273
     is set.
3b. **Beds and scenery use.** Done.
   - Using scenery (the same `UsePortal` request as doors: the PC walks there first) runs its
     use script.
   - A bed (art category 22, 0x428c74) asks for the sleep window on it instead (`BedRequests`,
     `arcanum_sim::rest`):
     - no town map check;
     - the bed's use script running the default refuses with `SleepUI.mes` 8;
     - another PC resting in it refuses with 10;
     - otherwise the window opens for sleeping, and each hour heals twice.
   - The host keeps what the window was opened with (`RestOffer`).
   - **Client:** a click on a bed, or on scenery with a use script, walks there and uses it.
     Other scenery lets the click through to the ground (builder's call, so trees do not swallow
     walking).
   - **Data:** all beds are sector objects. The start map has 271 beds, 47 of them with a use
     script; Bates Mansion's beds all have one; most other maps have none (so their beds
     refuse).
   - **Not built:** hiding the PC in the bed and moving it off the bed's tile (0x45f2d0); NPCs
     in beds.
4. **Messages and time** (inn beds). Done.
   - Action 0x18 (0x44b390) shows line `operand 0` of the script's own dialogue file in the
     triggerer's message box. The file is `dlg/` beside `scr/`, with the same name and a
     `.dlg` extension (0x4cee60). In multiplayer it goes to that player only (0x4a2b10).
     Here it is `LedgerDelta::ScriptMessage`, in the text for the PC's gender.
   - `.dlg` files (`arcanum_formats::dialog`) have seven groups per entry: number, male
     text, female text, intelligence, test, response, result.
   - Actions 0x60 (day + 1), 0x61 (hour), 0x62 (minute) and 0x69 (hours since the start:
     `days * 24 + hour`, 0x45a890).
   - **Inn beds** (`01990Inn_Bed_Use.scr`):
     - the bed's counter 0 picks the inn;
     - global variables 1043-1052 hold the hour each inn's room is paid until;
     - "now ≤ paid" sleeps;
     - otherwise the script shows "You must pay the Inn Keeper to use this bed." (or "...
       again.") and the bed refuses.
     - Paying happens in the innkeeper's dialogue (not built).
   - **Real data:** 543 of the 2399 scripts now run to an end from their first line.
4b. **Teleports** (`arcanum_sim::big_move`). Same map built.
   - **The original:** action 0x2c (0x4481a1): objects (operand 0), map (a `Rules/MapList.mes`
     id, 5000 the start map), x, y. It queues the move (0x4d3380) and the run goes on; the move
     happens after (0x4d3330 → 0x4d3460 / 0x4d39a0): followers come along to the same tile, the
     tile is used as given, the camera recentres. 0x66 (0x449a85) is the same with a fade, sound
     and delay first (operands 0-2; objects 3, map 4, x 5, y 6); 0x67 a fade alone. 0x66 and 0x67
     are skipped in multiplayer (`[0x62ac6c] & 1`).
   - **Data:** 457 teleport actions in 357 scripts; the object is set 0 (the triggerer) in 440,
     1 in 8, set 21 in 9. Maps 5000-5075 and more; 5000 in 157.
   - **Here:** the run does not wait (as the original). A teleport of a PC within the loaded map
     is put to every player's vote (`plans/multiplayer.md`); with one player it happens at once.
     The PC jumps (`Teleported`), standing; its position is saved.
   - **Found on the way:** object sets 0 and 1 were the wrong way round (0 is the triggerer).
   - **Other maps** (built next, `feat/map-switch`): the whole party goes, after the same vote.
     The old map's changes go into `WorldSave::other_maps`, the new map's come out; the old map's
     NPCs despawn, the new map opens around the tile and its NPCs spawn as its sectors load.
     Clients get `EnterMap`, the map's changes and `Teleported`, and clear the old map's
     sprites. Talks end; barter and loot windows on the old map close on the host.
     Real data: a teleport to map 5001 opens `BessieToonesMine-fixed` with 16 NPCs round the
     tile.
   - **Not built:** teleporting NPCs and objects; followers; fades and delays (0x66 teleports
     without them, 0x67 does nothing); votes in the save; the client's open dialogue, barter
     and loot windows are not closed by the switch.
   - **Jump points** (built next, `feat/jump-points`): `maps/<map>/map.jmp` (loader 0x4e30a0 /
     0x4e3800): a `u32` count, then 32-byte entries (flags, pad, source x/y, map, pad, destination
     x/y); the map is `id - 4999`, -1 this map; the flags are never read. A PC placed on a
     source tile (0x444270 → 0x4101d0 → 0x4e3450; each step ending there) queues a teleport
     (0x4d3380, no fade); NPCs and followers do not. Here it goes to the same vote. Data: 45 jump
     points on 17 maps, 22 to the same map; every destination map is listed. Stairs and doors
     to other maps are script teleports on their "use" (the `_TEL` scripts), built above.
4c. **Quests, gold, items, calls and the vicinity** (built, `feat/script-gaps`; names from
   `semes/condition.mes` and `semes/action.mes`, whose numbers are the types).
   - **Conditions** (0x4451b0; over a set: count 0 is false, else all must pass, or one for
     the "anyone" sets):
     - 2 gold at least (0x464780); 6 a PC's quest state exactly (0x4c4cb0: botched, flag
       0x100, reads 6; not a PC, the global quest state: not built, 0); 0x11 carries an item
       whose `name` is N (0x462480); 0x14 is named N and not `flags` 0x8000000; 0x16 dead
       (0x45d8d0: hit points below 1; nothing takes damage yet, so never); 0x2f identified
       (0x464370); 0x32 items being rewielded (never here).
   - **Actions** (0x447226):
     - 0x10 set a PC's quest (0x4c4d20): only upwards, never from 4, 5 or 6. Not built: the
       global quest states (2 → 4/5/6), the quest XP at 4, the giver's reaction (at least 41 at
       2, +10 at 4), the timestamp.
     - 0x17 float a line of the script's dialogue over each object (0x44b1f0), to every player;
       the original's duration and "a PC near" check are not built.
     - 0x22 object type (raw `obj_f_type`); 0x23 gold (0x464830: a take larger than the gold
       does nothing); 0x29 move the first item named N (0x4617f0; no room, it stays); 0x42 make
       an item of basic prototype N inside (no room: the original leaves it on the floor; here
       it is not made).
     - 0x51 call the script at another object's point P from line L with triggerer T (0x44b170
       → 0x441980; result ignored, attachment kept; nested up to the runner's depth); 0x63
       change a script (the running one's number if it is the same object and point).
   - **Sets:** 8/9 everyone / anyone in the vicinity: PCs and NPCs round the attachee
     (0x440fc0, mask 0x18000; the original's area is a view-sized rectangle, here 10 tiles);
     0x15 the player: the acting player's PC.
   - **Host:** scripts change PCs' inventories in the save; `sync_script_inventories` copies
     them to the PCs and sends them. NPC and container inventories are kept in the sector's
     changes.
   - **Real data:** from their first line, 1731 of 2399 scripts now run to an end (1085
     before). Next most common stops: action 0x5b (119, magic/tech adjust), 0x7 (35), 0x1d (24,
     reaction), 0x21 (21), 0x36 (20).
4d. **Calls, timers, stats, rumours, the rest of the vicinity** (built, `feat/script-gaps-2`).
   - **Actions:** 0x7 call script N from line L for each triggerer × attachee (0x44b030: a
     record of its own, not kept; the caller's point); 0x36 the same once N × 8 game seconds
     have passed (a time event of type 4 on the game clock; here `TimedScripts`, not in the save
     yet; one triggerer and attachee); 0x1d store the reaction (0x4c0cc0); 0x21 store an
     effective stat (0x4b0490; NPCs: their base stats); 0x32 a PC knows rumour N (0x4c57e0);
     0x33 rumour N quelled for everyone (0x4c5690, bits 0x6876c8); 0x5b magic/tech adjust
     (0x461540: `(F × N + 50) / 100`, F = 100 × effective / complexity, effective = half of
     aptitude plus complexity, kept between 0 and the complexity).
   - **Conditions:** 0xa met before (0x4c0c40); 0x21 has reputation; 0x28 heard rumour; 0x29
     rumour quelled; 0x30 knows spell (college level above the spell's place).
   - **Sets:** 0xf/0x10 scenery, 0x11/0x12 containers, 0x13/0x14 portals, 0x16/0x17 items in the
     vicinity (masks 8, 4, 2, 0x7fe0), all within 10 tiles here.
   - **Not built:** 0x31 / 0x54 areas (next PR); 0x38 toggle off (`flags` 2, no object flag
     changes are kept yet); 0x3a kill (nothing takes damage); 0x52 play sound (the id-to-file
     mapping 0x41a940 not decoded); the temporary record's header defaults (0x44c710).
   - **Real data:** 1875 of 2399 scripts run to an end (1731 before). Next: action 0x4d (82),
     0x50 (47), 0x31 (18), condition 0xb (18), action 0x38 (17), 0x52 (17).
4e. **Reputations, reactions, distances; no-ops that are faithful** (built,
   `feat/script-gaps-3`).
   - Actions 0x48 / 0x49 give / take a reputation; 0x1e / 0x1f set / adjust a reaction
     (0x4c0de0: the stored part moves by the difference); 0x47 the distance between two
     objects (0x4b96f0 not read: the straight line in tiles).
   - Condition 0x31 mastered a college (all five spells).
   - Faithful because nothing of the kind exists yet: action 0x4e removes an effect (none
     exist); conditions 8 / 9 bless / curse and 0xb bad associates are false.
   - Not shown yet, the script goes on: 0x52 sound, 0x78 eye candy, 0x83 slides.
   - **Real data:** 2005 of 2399 scripts run to an end (1906 before). Next: action 0x4d give an
     effect (82), 0x50 magic/tech adjust on a target (48), 0x38 toggle off (17), 0x3a kill
     (17), 0x28 transform (12), 0x72 plaque window (12), 0x3b art (11).
4f. **Effects** (built, `feat/script-effects`).
   - **Note:** the action dispatch is `dec eax; jmp [0x44adbc + 4 × eax]` (0x447248): action N
     is table entry N - 1.
   - **Action 0x4d** (0x449190 → 0x4e9f70): objects (operand 0), effect id (1), cause (2). Critters
     only; appended to the effect ids (field 0xe4) and causes (0xe5), no limit, duplicates kept.
     **0x4e** (0x449230 → 0x4ea100): objects, id; drops the first hold of the id, whatever its
     cause.
   - **Reading `rules/effect.mes`** (0x4e9c20): tokens split at spaces and commas; keys compared
     without case against five tables (stats 0x5b9ba8, skills 0x5b9c18 / 0x5b9c48, resistances
     0x5b9c58, expertise 0x5b9c6c, others 0x5b9c8c: maxhps, maxfatigue, ...); values `+N` / `-N`
     (type 0; `%` at the end: type 5), `*N` (1), `/N` (2), `min N` (3), `max N` (4). An unknown
     key ends the line.
   - **Applying** (0x4ea6c0, for every effect held): percentages summed, then multiplied, divided,
     additions summed, the highest `min` a floor, the lowest `max` a cap, in that order. Used by
     the effective stat (0x4b06f7), max hit points (0x43d5e2), max fatigue (0x45d6e4),
     resistances (0x43d7ee), skills (0x4c5f1e / 0x4c662d).
   - **Here:** PCs hold them on the sheet (`CharacterSheet::effects`, saved); race, gender and
     background effects go through the same loop. A changed sheet goes to its player, and the
     PC's status to everyone. Fatigue spent is kept as damage, so a new maximum moves the current
     value as the original's fix-ups (0x4e9f70) do.
   - **Not built:** NPCs' effects (not kept; their stats are their prototype's base stats); the
     stats' own limits after the effects (0x4b0fc0 / 0x4b0fd0); `magicpts`, `techpts`,
     `aptitude` and `alignment` count only `+N` / `-N`; expertise and the other keys (reaction,
     criticals, `xpgain`) are read but nothing uses them yet.
   - **Real data:** all 309 effect lines read whole (651 changes). 2085 of 2399 scripts run to an
     end (2005 before). Next: 0x50 (48), 0x38 (17), 0x3a (17), 0x28 (12), 0x72 (12).
4g. **Magic/tech on a target, switching objects** (built, `feat/script-gaps-4`).
   - **Action 0x50** (0x44933f → 0x461620): N (operand 0), item, user and target (1-3, single
     objects), result (4) := `N × (100 - r) / 100`. r is 0 unless user and target are critters
     and the item's adjustment k (field 0x60) opposes the target's aptitude: magick (k > 0) on a
     technologist loses the target's whole aptitude (-target) when the user is a technologist
     too, else `(100 - user) × -target / 100`; tech on a mage the same, mirrored.
   - **Actions 0x38 / 0x39** (0x4487ce / 0x448858): for each object (operand 0), flip object
     flag 0x2 "off" (0x39: 0x400000; 0x4efee0 clears, 0x4eff50 sets), any type. Here kept in the
     sector's changes (`object_flags` / `mobile_flags`, saved), applied to the loaded object
     (blocking follows), sent to every player (`ObjectFlags`); the client draws the sector
     again. An NPC switched off despawns for everyone; switched on, it spawns. NPCs off in the
     map's data are not spawned.
   - **Not built:** objects off in the map's data are not loaded as mobiles, so they cannot be
     switched on; most 0x38 calls are in heartbeat scripts, which do not run yet. The original's
     clean-up when switching off (0x4d62b0 / 0x4d56c0 for flags 0x8 and 0x200000) and its light
     updates.
   - **Real data:** 2105 of 2399 scripts run to an end (2085 before). Next: 0x3c (29), 0x3d (26),
     0x3a kill (17), 0x28 (12), 0x72 (12).
4h. **Damage, spells, transforms, text, art, skills, destroy, switches** (built,
   `feat/script-gaps-5`).
   - **0x3c damage** (0x4489f9 → 0x4b2210 / 0x4b4390): objects, amount, kind (0 normal, 2
     electrical, 3 fire: hit points; 1 poison, stat 0x18; 4 fatigue). PCs take it on the sheet,
     hit points down to 0. NPCs (`feat/npcs-spring-traps`) in the sector's changes: poison,
     fatigue, hit points; with none left they die as 0x3a kills (builder's call: no
     experience, no dying script). Not built: armour and resistances, PCs' death.
   - **0x3d spell on a target, no caster** (0x448ab7 → 0x455a20 / 0x455ac0): here Minor /
     Major Healing (5 / 40, no aptitude) and Halt Poison on PCs; other spells not built (the
     script goes on).
   - **0x28 transform** (0x447e89): the script's own object becomes a new object of prototype N
     in its place (0x4ed8b0: only its location stored). Kept (`object_protos` / `mobile_protos`,
     applied before the other changes, which it clears), sent (`ObjectTransformed`), the sector
     redrawn, an NPC spawned again. Not built: keeping the hurt fraction, rotation, the carrier
     of an item.
   - **0x72 plaque / 0x40 book** (0x44a15d / 0x448ba4 → 0x4ee290 → 0x56bc00): to the PC's
     player's message box, a paragraph a line (builder's call: no book or plaque window yet);
     `gameplaque.mes` one entry, `gamebook.mes` entries to the first gap; the font and
     justification token left out.
   - **0x3b art number** (0x448947 → 0x502780, frame 0, kept only if the art exists): unique
     NPCs 8 bits at 20, items 11 at 17 (the limit 0x502830 not read: 2048), interface 12 at 16,
     others 9 at 19; tiles, walls, critters, monsters none. Kept (`object_art` / `mobile_art`),
     sent (`ObjectArt`), redrawn.
   - **0x2f skill level** (0x448374): PCs the rules' level; NPCs points × 4 up to 20 (their stat
     cap not applied).
   - **0x44 destroy** (0x448d19 → 0x43cca0): flag 0x1 (hidden, not blocking, NPCs gone). Not
     one destroyed already (0x43ccfa); its destroy event (2) first, itself the triggerer and
     attachee (0x43cd0e), "skip" keeping it (0x43cd18; `feat/destroy-event`). Real data: eight
     objects of the start map's loaded sectors attach it (containers, generic items, an NPC).
   - **0x6e / condition 0x2e**: flip / test entry N of the 256-entry table 0x5fc350 (bit 0x80),
     kept as `ScriptGlobals::switches`. Its other users read objects' bits 21+ (probably monster
     generators; not traced).
   - **Conditions:** 0x7 a global quest state is N (0x4c51a0; none are kept: 0); 0x15 wears an
     item named N (slots 1000-1008); 0x1f a critter with `critter_flags` 0x40000 (meaning not
     traced).
   - **Real data:** 2232 of 2399 scripts run to an end (2105 before). Next: 0x3a kill (17),
     set 0xd (9), traps (8), 0x11 / 0x15 (6).
4i. **Item wield scripts** (built, `feat/item-scripts`).
   - **Events** (0x441980's 72 callers): 0 examine, 1 use, 2 destroy, 3 unlock, 4 get, 5 drop,
     6 throw, 7 / 8 hit / miss, 9 dialogue, 10 first heartbeat, 11 catching a thief, 12 dying,
     13 / 14 enter / exit combat, 15 / 16 start / end combat, 17 buy, 18 resurrect, 19
     heartbeat, 20 leader killing, 21 insert item, 22 will kill on sight, 23 taking damage, 24
     / 25 wield on / off, 26 critter hits, 27 new sector, 28 remove item, 29 leader sleeping, 30
     bust, 31 dialogue override, 32 transfer, 33 caught thief, 34 / 35 critical hit / miss
     (several names from the standard list, not decoded).
   - **Wield** (0x466640 → 0x4677b0: event 24 after the item is in slot 1000-1008; 0x467860 →
     0x467cb0: event 25 after it left; triggerer the holder, attachee the item; the answer is
     ignored). Rewielding all (0x465530: 25 then 24) happens only when base stats 22 / 23
     change; condition 0x32 reads that flag. Loading a game runs nothing.
   - **Here:** `run_wield_scripts` compares each PC's worn items with `PlayerSave::worn` and
     runs the item prototype's scripts (`ScriptObject::Carried`), taken off first. A new
     character's starting gear is put on once. NPCs' wield scripts, events 21 / 28 and the
     rewield on stats 22 / 23 are not built.
   - **Action 0x7d** (0x44a5f8): cancel script N's scheduled runs (0x36) for an object
     (0x45bff0, type 4, 0x44aff0 compares the record's script and an object; which object was
     not read: either here).
   - **Real data:** 27 item scripts on events 24 / 25; all run to an end. 2235 of 2399 scripts.
4j. **Heartbeats and killing** (built, `feat/heartbeats`).
   - **Heartbeats** (time event type 10, 0x4ad200; `arcanum_sim::heartbeat`):
     - NPCs only, on loaded sectors, switched off or not.
     - The first beat comes as the sector loads (0x441fc0 → 0x4ad790). It runs event 10;
       unless that says "run the default", event 19 is skipped that time.
     - Event 19 runs while a PC is within 30 tiles (0x4ad420). The NPC is triggerer and
       attachee.
     - The next beat comes after `250 + 4750 × d / 30` ms (0x4ad5d0, checked:
       0x88888889 divides by 30), plus 0-5000 ms after the first.
     - "Skip the default" holds the NPC's wandering until the next beat.
     - **Not built:** turn-based combat's per-turn beat (0x4b7010); corpse clean-up.
   - **0x3a kill** (0x4488e8 → 0x45d900):
     - NPCs are kept dead (`SectorDelta::dead`) and shown on the last frame of the death
       animation (7), as 0x45d900 leaves them.
     - They are spawned again as corpses: no wandering, no heartbeats, no talk (builder's call
       for talk).
     - A PC loses all its hit points; death is not built.
     - `is_dead` reads both.
   - **Real data:**
     - On the start map, 13 NPCs have heartbeat scripts. The crash site's 9 kill themselves at
       the first beat (scripts 1781-1792: unless global flag 2350 is set). The other 4 beat
       with nothing unbuilt.
     - 2252 of 2399 scripts run to an end.
4k. **Followers, parties, quests, blessings, pools** (built, `feat/script-gaps-6`).
   - **Sets** (0x44b4e0): 2/3 the triggerer's followers (field 0xf1 by 0x4411c0: not those
     waiting, not the triggerer); 0xd/0xe the same with the triggerer last; 4/5 its party
     (0x441310; alone, the triggerer); 6/7 a PC's party with each member's followers (0x4413e0;
     not a PC, itself). Here a PC's party is every PC in the world (owner's team rule). The sets
     are read when the run starts (the original reads them at each use).
   - **Conditions:** 6 on a non-PC reads the global state; 7 the global quest state; 8 / 9 has
     blessing / curse N (a PC's list); 0x19 surrendered (an NPC with `critter_flags` 0x4000,
     0x4af210); 0x1a in a conversation; 0x1c sees (0x4af260; here: alive and within its
     perception, 1-10 tiles, the larger distance; light, invisibility and walls not copied);
     0x22 within N tiles of X, Y (0x4b96f0: the larger of |dx|, |dy|); 0x23 under spell N
     (0x459380; no spells are kept on critters: never); 0x2d the sector at X, Y is in the list
     at 0x601794 (0x4d0ee0; who fills it is not traced: never, builder's call).
   - **Actions:**
     - 0x11 global quest state (0x4c51c0, table 0x5ff418): only 4 and 6 are written, and never
       over either (`ScriptGlobals::quests`).
     - 0x15 follow a leader by force (0x45de90 with 1: no checks; `npc_flags` +0x400, -0x20);
       0x16 stop following (0x45dfc0 with 1). Both go through `follow::set_follower`, as
       dialogue does.
     - 0x19-0x1c add / remove a blessing or curse (0x4c42f0 / 0x4c4450 / 0x4c3e40 / 0x4c3fa0):
       PCs only, ids from 50; the effect is entry `N × 10 + 1` of `mes/gamebless.mes` /
       `gamecurse.mes` (0x4c4170). 16 of 18 blessings and 14 of 17 curses name an effect of
       `rules/effect.mes`; the others name 0, 4 or 8, which are not there. Their times and
       messages are not kept.
     - 0x20 the art's armour nibble (0x504030: `current_aid` type 2 bits 20-23, type 0xc bits
       20-22; PCs have no art id here: 0). Not armour class.
     - 0x2d an NPC's day standpoint (field 0x121), kept (`SectorDelta::standpoints`); the NPC is
       spawned again with it.
     - 0x4b / 0x4c heal hit points / fatigue (0x4b58c0 / 0x4ed720): PCs' sheets; NPCs' damage
       is not kept. 0x6b / 0x6c store current and maximum hit points / fatigue: PCs from the
       rules; NPCs from their data (`hp_pts + hp_adj`, the critter part 0x43d630 not added;
       fatigue `pts × 4 + adj + 4 + (level + CN) × 2 + WP`, 0x45d670).
     - 0x75 one fate point (stat 0x14, PCs); 0x89 adjust a stat (0x4b0980) within tables
       0x5b5194 / 0x5b5204: here PCs' primaries (1-20), alignment (±1000), fate (0-100), poison
       (0-1000); other stats and NPCs are not changed.
     - 0x77 unbotch (0x4c5070): only when the global state is 6; then it is 2 and the PC's
       record loses its botched bit (`PcScriptState::botched`).
     - 0x7b touch art (0x4ef190): an art-cache load; nothing.
   - **Real data:** 2328 of 2399 scripts run to an end (2252 before). Next: traps (8), 0x24
     combat attack (6), 0x88 (5), 0x34 / 0x5a / 0x5d / 0x7f (4).
4l. **Traps** (built, `feat/traps`; `arcanum_sim::trap`, the runner's `trap`).
   - **What is a trap** (0x4bbe00): scripts 30000-30006 whose header has flag 1 (mechanical) or
     2 (magical). In the data: 30000 magical (0x6), 30001-30006 mechanical (0x5), all run once
     (flag 4); 30007, "Trap source", has flags 0 and is not one. The scripts have no lines.
   - **The hook** (0x441980): a use (event 1) of an object whose trap is not spotted makes a PC
     roll Spot Trap (0x4bbe40, skill 7: a basic skill, 0x4c62e0, 4 a level, 0x4c6350; corrected
     in `feat/spot-chance` from Pick Locks' 6); spotting it cancels the use. Otherwise, spotted or not,
     the trap springs (corrected 2026-11-25: the agent's "spotted cancels" is wrong; 0x441980
     rolls only when 0x4bbfe0 says not spotted). Who knows a trap (0x4bbfe0, `knows_trap`,
     `feat/npcs-spring-traps`): a PC or an NPC with a leader when it is spotted or a PC set it;
     an NPC without one unless a PC set it. One that does not rolls its own Spot Trap
     (0x4bbe40 takes types 15 and 16).
   - **Springing** (0x4bcb70 / 0x4bcdc0): `mes/trap.mes` 1 "Trap!" over the triggerer; the
     victims are the triggerer, or the critters within the header's radius (0x20 / 0x40 / 0x80:
     1 / 2 / 4 tiles; no trap in the data has one); each takes `random(byte 0, byte 1)` of the
     attachment's counters as damage of the trap's kind (record field 0x11 + kind, as action
     0x3c: 30001-30003 hit points, 30004 fire, 30005 electrical, 30006 poison; 30000 none), then
     spell byte 3 if set. Run once: the attachment's number becomes 0. The use does not happen.
   - **Spot roll** (0x4c7160 case 7): the skill chance (0x4c69f0, as Pick Locks); a Master who
     fails rolls again; a success below Expert on a magical trap fails. Spotted: `flags`
     0x2000000 (kept, sent; the original keeps it per PC in multiplayer), `mes/trap.mes` 0.
   - **Disarming** (`PlayerIntent::DisarmTrap`, skill 15, 0x4c7160 case 0xf → 0x4bc7b0): doors
     and containers, a spotted trap only; the lock pick's roll with no difficulty (0x4c8430
     adds `trap_difficulty` only for trap objects); a Master who fails without a critical
     failure rolls again. Success: the script number becomes 0 (0x4bc220); a critical failure
     springs it on the PC (0x441980 event 1). `mes/skill_ui.mes` 560 / 561.
   - **Spotting while walking** (built, `feat/walk-spotting`, `feat/followers-spot-traps`;
     0x427110 each step, any walker): the walker rolls Spot Trap (0x4bbe40) for each trap object
     (type 17, list mask 0x20000; not doors or containers) on the tile its next step enters
     that it does not know (0x4bbfe0), in list order; the first spotted stops the walk before
     the step (0x4272b3 → 1). 0x4bc090 marks it (`flags` 0x2000000) only for a PC (the local
     one, 0x4bc106; here any) or an NPC with a leader (0x4bc125); a leaderless NPC (which
     knows every trap a PC did not set, so rolls only for those) stops and marks nothing. The
     PC is told `mes/trap.mes` 0 (builder's call: the original's UI event 4, 0x460630, not
     traced); followers say nothing. Once a tile per walk here (the original rolls every step
     attempt; a failed roll lets the step go on, so the same). NPCs roll with their own skill
     and Intelligence and the light of the tile ahead. Not built: the local-PC bypass 0x458a80
     (`0x14` bit 0x100, probably a cheat).
   - **Stepping onto a trap object** (built, `feat/step-on-traps`; 0x4305d0 at 0x430ca8-0x430d0f):
     after a step onto a tile with a trap object (list mask 0x20000, the first only) the walk
     ends there (0x430cf7) and the walker uses it (event 1, 0x430ce3: triggerer the walker,
     attachee the trap; the answer ignored). The use runs the trap as above: the Spot Trap roll
     when not spotted (a success keeps it, 0x4419d6), else it springs. After the run a trap
     object whose script number is now 0 is destroyed (0x441a5f-0x441a80, field 0x14f the
     type 17; through 0x43cca0, so its destroy event too). Any walker, PC or NPC
     (`feat/npcs-spring-traps`), but one of `spell_flags` 0x2 / 0x4 (0x430ca8, read at
     0x43086f; the spells not identified). An NPC's use runs for the first player.
   - **Not built:** the arrow
     trap's attack roll (0x4b3170 from its source object, 30007); recovered parts on a critical
     success (0x4bc9c0, table 0x5b5f80); setting traps (0x4bc480, table 0x5b5f60); the eye candy
     (`rules/TrapEyeCandy.mes` N × 10 + 0-2); the client's way to arm Disarm Traps (the bottom
     bar's skills view, next).
   - **Real data:** 2336 of 2399 scripts run to an end (2328 before).
4m. **Item and lock events** (built, `feat/item-events`).
   - **Get (4)** (0x461810, before the pick-up): triggerer the picker, attachee the item (a map
     item's own attachments, a dropped one's prototype's); "skip the default" leaves it.
   - **Drop (5)** (0x461ab0, before): triggerer the holder; "skip the default" keeps it.
   - **Insert (21) / remove (28)** (0x466640 / 0x467860, after; the answer ignored): here a
     host system compares each PC's items by prototype with the last look and runs remove, then
     insert, whatever moved them; a PC seen first runs nothing. Builder's call: a move inside
     the inventory runs nothing (the original's moves inside may run them; not traced); NPCs'
     and containers' are not run.
   - **Unlock (3)** (0x4aeb70 doors, 0x4aed80 containers): a try at the lock, not a successful
     unlock. Doors: after the use script, when opening a shut door, before the lock check.
     Containers: before the use script (0x572370). "Skip the default" denies the opening (the
     original's result 4; its message is not shown). Picking, keys and the Unlocking spell do
     not run it.
   - **Not built:** examine (0, 0x57cd60: a PC only, before the description; "skip" shows
     none) — there is no examine yet. Destroy (2) runs for
     action 0x44 (`feat/destroy-event`); the original's other 56 callers of 0x43cca0 (a
     weapon destroyed in a fight, an item worn to nothing, 0x43f1c0) do not run it here.
4n. **The written window** (built, `feat/written-windows`; client `written_window`).
   - Actions 0x40 (book), 0x41 (image: the number is its interface art) and 0x72 (plaque) send
     `LedgerDelta::Written { kind, line }` to the reader's player (they went to the message box
     before). Kinds (0x56bc00): 0 book, 1 note, 2 newspaper, 3 telegram, 4 image, 6 plaque.
   - The client draws it as 0x56c050 does: art 494 over (0, 41, 800, 399), the kind's
     background (table 0x5ca4b8: 477, 489, 487, 491, -, -, 633) at (132, 0); entries "font
     justification text" (0x56c9f0: the font is an interface art number; `c` centres), stacked
     and wrapped; a book's two pages (0x5ca618) with previous / next (arts 495 / 496 at (213, 77)
     / (675, 77)); a plaque in (269, 40, 428, 327) in white; a note's entries joined in font 497
     in (349, 49, 215, 300). A click closes it (Escape too, builder's call).
   - **Newspapers and telegrams** (built, `feat/newspapers`): a paper is `mes/gamenewspaper.mes`
     keys: its header (table 0x5ca638: font, key, x, y, alignment), the headline (the number)
     in (186, 97, 255, 90), then up to nine stories flowing down four columns (0x5ca738), then
     filler stories from keys 10 on, starting at number / 10 + 10, round the ones there. A
     telegram (`mes/gametelegram.mes`): eleven labels (0x5ca538, keys 1000-1010), the notice
     (1011, font 481) and the entries from its line joined in font 480 (0x5ca508).
   - **The newspaper queue** (module "Newspaper", 0x4bf060-0x4bf330; `arcanum_sim::newspaper`,
     saved in the script state): four past papers, today's, up to twenty waiting; 5000 at the
     start. Action 0x55 queues (first in line with a priority; scripted papers below 5000 not
     twice); each game day the next comes out, or a random stock paper
     `(rand(0, m) + 500) × 10`; action 0x56 floats today's headline. Builder's call: a new paper
     each game day (the original's event every 24 hours from the start); which paper a bought
     or found newspaper item shows is its own line (the `P:` seller not traced).
   - **Not built:** the alternate background 817 and header (0x56bb60, map 0x36f8); the sounds.
   - **Reading items** (built, `feat/read-items`; 0x56bb60): a written item (type 13) released
     on the inventory's use box, or right-clicked when not looting (builder's call: the
     original's use), opens the window on its `written_subtype` (schematics, 5, not here) from
     its first line: the item's own `written_text_start_line` (carried as
     `CarriedItem::written`, kept through the ground and inventories), else its prototype's.
     Action 0x5e sets it on a carried item. In the data, 10 written prototypes are readable
     (58 are schematics); most lines are on the placed items.
4o. **Script gaps 7** (built, `feat/script-gaps-7`).
   - **Actions:** 0x2e night standpoint (field 0x122, kept as `SectorDelta::night_standpoints`);
     0x34 make an object of basic prototype N near an object (0x4ed8b0; here items only, on its
     tile as a drop; the original tries ten random tiles 1-3 away and makes anything); 0x3f
     quest experience (`Rules/xp_quest.mes` N, 0x4c53c0 → 0x45f110: PCs, shared as dialogue's);
     0x46 the wielded weapon's art nibble (slot 1004, field 0x5f bits 6-9 of an item art; none:
     1); 0x59 an NPC's faction (0x124); 0x5a the scroll distance (the player's PE / 2 + 3);
     0x5f an object's X and Y; 0x6a toggle a sector in the blocked list (key 0x4cfc50: 64-tile
     sectors; kept as `ScriptGlobals::blocked_sectors`; condition 0x2d reads it; the original's
     paths avoid those sectors, not built); 0x6f an armour's coverage (field 0x5d bits 14-16;
     not an armour: -1); 0x80 / 0x88 an unresistable spell (0x3d's with flag 4).
   - **Go on without effect:** 0x53 / 0x57 sounds, 0x68 / 0x74 / 0x7c eye candy, 0x5d prone (the
     knock-down pose 0x435b30), 0x71 unfog a town map (no town maps), 0x7f an item shown or
     hidden in inventories (field 0x57 bit 0x10; carried items keep no flags).
   - **Conditions:** 0x1d hears (0x4af470: within `(clamp(PE - 4, 1, 10) + 6) / 2` tiles;
     obstruction not counted; prowling counted since `feat/prowling`, 4s); 0x24 a portal or container is open.
   - **Real data:** 2377 of 2399 scripts run to an end (2336 before). Left: 0x24 combat attack
     (6), 0x45 walk to (3), 0x55 newspaper queue (3), 0x4f use a skill (2), 0x5e written start
     (2), 0x84 end game (2), 0x3e animation, 0x56 headline, 0x5c rename, condition 0x13.
4p. **Script gaps 8** (built, `feat/script-gaps-8`). Operands checked against the handlers.
   - **Actions:**
     - 0x45 walk to X, Y (0x448d48 → 0x4341c0, goal 7): object, X, Y. The critter (PC or NPC)
       walks there on a path from where it stands; none found, it stays. The script goes on. An
       NPC's idle thinking takes it back to its standpoint afterwards unless a script moved that
       (script 1378 does both).
     - 0x4f use a skill (0x4492a1 → 0x4352c0, goal 0x28): actor, tool, target, skill, a number.
       Built for Pick Locks (14) by a PC on a door or container: the pick a player would make,
       with the PC's first item of the tool's prototype as the tool (its own bonus counts; the
       fifth operand is not used). Other skills do nothing yet (builder's call). Its users are
       the item scripts 29009 / 29010.
     - 0x5c rename (0x449782 → 0x4efdd0): sets the object's `name` (field 0x16), the number the
       item-named actions and conditions match (the hover shows `description`, 0x17). Map
       objects only, kept as `SectorDelta::object_names` / `mobile_names`.
     - 0x3e play animation N (0x448b10 → 0x4332e0; N < 26; 10 on a critter queues goal 0x3c):
       not shown; the script goes on, as in the original.
     - 0x84 end the game (0x44a8ce → the interface callback 0x460530; a host also sends packet
       0x7c): kept as `ScriptGlobals::game_ended`; the ending is not shown.
   - **Condition 0x13** (0x445b72): a set member's `aid` (field 0x18) is monster art (type 0xc)
     of number N (bits 23-27, 0x503f20); all or any by the set's kind.
   - **Real data:** 2393 of 2399 scripts run to an end. Left: 0x24 combat attack (6: the set
     turns hostile to the object, 0x4a9650; built with combat, `plans/combat.md` step 4: all
     2399 run).
4q. **Pick Pocket** (built, `feat/pick-pocket`; `arcanum_sim::pickpocket`, 0x4c7160 skill 5).
   - The skills view's Pick Pocket button arms it; a click on an NPC walks there
     (`PlayerIntent::PickPocket`) and opens the barter window on the NPC's own inventory without
     prices (`BarterView::pickpocket`). Refused targets (`critter_flags2` 0x80, mechanical
     `critter_flags` 0x20000000): `skill_ui` 525.
   - Dragging an item out rolls: chance from table 0x5b7068 by level − the victim's Perception
     (quartered, at most 95), difficulty from slot table 0x5b70e4 and cells × 5 (halved from
     Expert), the roll as Pick Locks'. Gold 100 and ammunition 20 at a time. `skill_ui` 500 / 501.
   - Caught (critical failure, or untrained and the victim's Perception roll, its die a
     builder's call 1-20): the window closes (`LedgerDelta::BarterClosed`), the victim's event 11
     runs; unless it skips the default, reaction -10 (builder's call; the original turns it
     hostile, 0x4a9650).
   - Not built: planting, the victim's awareness and the thief's state modifiers, critical
     success, event 33, the stolen flag.
4r. **Repair** (built, `feat/repair`; `arcanum_sim::repair`; 0x4c7160 case at 0x4c7bce, callbacks
   0x6876b4 / 0x6876b8).
   - No damage roll: success is forced unless the skill's chance (0x4c69f0) is below 1. The hit
     points restored are max × chance / 100 (at least 1; all of them with effect 9); the most
     falls by 10 / 5 / 1 / 0 % by training (`hp_adj`), and a Master mends a broken item (art bit
     0x400) for 5 %. Undamaged: `skill_ui` 505 "That is not in need of repair."; else 500 / 501.
   - Built: carried (and dropped) items keep their wear (`CarriedItem::wear`); critical hits
     wear armour and weapons (`plans/combat.md` step 9). The skills view's Repair button opens
     the inventory and a click on an item sends `InventoryRequest::Repair` (builder's call:
     the original's target cursor). The chance is 0x4c69f0's Repair row, 4 a level
     (`tech_chance`; checked: the table 0x4c6ad8 also corrects Firearms to 5 a level + 25 and
     Disarm Traps to 4 + 20).
   - Not built: a Master mending broken items, effect 9, repairing items on the ground or
     others'.
4s. **Prowling** (built, `feat/prowling`; `arcanum_sim::prowl`; 0x57a0a0 → 0x45ee30 →
   0x45ee90).
   - The skills view's Prowling button toggles it (`PlayerIntent::Prowl`); a right click in the
     world stops it (0x54de50). The PC carries `Prowling` and its art takes animation 5 (0x45efa0;
     off: 0), told to every player (`LedgerDelta::Prowling`, and later players by the `Spawn`'s
     art). Not saved (builder's call).
   - Its value (0x4c62e0 skill 6): 0 untrained by level, else level × 5 + 25. Condition 0x1d
     (0x4af470): a listener's Perception loses the prowler's value in percent before the reach
     `(clamp(PE - 4, 1, 10) + 6) / 2`.
   - Not built: sight (0x4af260), NPCs noticing prowlers (0x4aa0d0), followers copying it, the
     hover's prowling state, untrained prowlers' path flag 0x200.
5. **Gold and items:** done in 4c.
5. **Float text and messages** (0x40/0x41/0x72) as deltas to the right players.
6. Later: dialogue (4, 0x30, event 9), quest state, reputation, followers, timed events,
   teleports, combat-related actions, the remaining conditions.

## Open

- What decides the start line (B+4) besides 0. The theft checks for event 1. Attach point
  names (no table in the exe; 9 is dialogue).
- Script flags other than 4, 8 and 0x10. The 8 ignored header bytes.
