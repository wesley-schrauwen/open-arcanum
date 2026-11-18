# World milestone: networked world from a save

Status: design approved 2026-11-13, not implemented.

## Intent

The game is multiplayer first. After the player picks or creates a character, the "The world goes
here" placeholder (`screens.rs`, `AppState::InGame`) is replaced by a real world. The PC stands on
the original starting map and walks where the player clicks.

The world is built on a host/client split from the start, with a hybrid trust model (below): the
client acts at once on its own PC and the host validates, corrects and owns everything shared.
Single player is the same game with the host and client in one process over a loopback transport.

Why this order: adding host authority to a single-player game later means rewriting every system.
Building the netcode before the world exists means guessing at messages that real state would
define. So the first world state goes through the split, and nothing is networked before there is
state to network.

## Goals

- **Milestone A:** the PC on the starting map, with its static scenery (walls, buildings, trees)
  drawn depth-sorted and blocking movement. The PC is animated and moves on click, entirely through
  `arcanum_sim` + `arcanum_net` over loopback.
- **Milestone A2:** a second client process over local UDP joins the host's world from a full
  snapshot, with its own character for that session. Two PCs walk around one world.
- The world always boots from a save. New Game writes an initial save; Load Game, Continue and
  the join snapshot use the same format and the same boot path.

## Non-goals (for these milestones)

- NPCs, containers, scenery interaction, dialogue, combat, scripts, quests.
- Client-side prediction, the tether, the inn, Steam transport, the host/join menus.
- Networked character creation. Creation and the character rules exist (see
  `docs/superpowers/specs/2026-11-15-character-rules-design.md`); until the host exists the
  client calls the shared rules directly in its place.
- Drop-out and reconnect at any time. The owner wants this, but its design is open.
- The lobby UI, session broadcasting, and the inn/campfire restriction (disabled for
  development). Character reset is designed for but not built.

## Anti-goals

- Client code that changes shared state or combat results directly. The only state a client may
  change first is its own PC's out-of-combat position (and the combat-start report); everything
  else comes from the host.
- Message types designed ahead of the state they carry.
- A save format that differs from the join snapshot.

## Owner's decisions

- **Two separate ECS worlds.** The sim has its own Bevy `World`; the client has another. They
  share only net ids and talk only through the transport. A remote client has only the client
  world, so single player and remote play run the same client code.
- **Hybrid trust, for snappy responses:**
  - **Out of combat, the client owns its own PC's movement.** A click paths and walks at once on
    the client. The client sends its path/position to the host, which checks it (walkable tiles,
    plausible speed, later the tether), then accepts and relays it to other players or rejects it
    and sends a correction.
  - **Any client may start combat, and combat is party-wide.** A client enters combat when it
    detects aggro locally or when its player chooses to. It enters at once and reports positions,
    and the host puts every party member inside the engagement radius into combat mode. Members
    outside it stay free, so players are not forced to adventure together; one who walks into the
    radius joins at the bottom of the turn order on the next round (as in `multiplayer.md`). The host checks them within a tolerance and builds the encounter,
    correcting positions during the combat-start pause if needed.
  - **In combat, the host is fully authoritative.** The client computes only the AP cost of an
    action and whether its character can make it (enough AP, reachable, in range), so it can start
    the attack, spell or movement animation at once. Everything else is host-side: hit and miss,
    damage, mana and fatigue costs, effects and final positions. The host recomputes the AP and
    legality too, corrects the client and returns the new state.
  - **Shared things are always host-confirmed:** picking up items, containers, doors, NPCs,
    dialogue, quest and world flags. Two players grabbing one item is resolved by the host only.

- Multiplayer first; the build order below.
- **Inns and campfires** are more prominent than in the original. They are where players save,
  where others join, and where a host broadcasts a multiplayer session. For development, a
  setting turns this restriction off: saving and joining work anywhere.
- **Joining:** a player can only join while the host is checked into an inn and sleeping. A join
  happens at a quiet point (no combat, dialogue or movement), so the host sends one full snapshot
  and normal deltas follow. The joiner's PC appears in the inn (or campfire). In A2 the
  restriction is turned off and the join is a "join now" stub.
- **Characters per session:** a player chooses Join from the lobby. If they have no character in
  that session, they create one or pick a pre-made. Characters belong to that session and are
  saved by the host in its `WorldSave`. A player can reset their character at any time, which
  discards it and returns them to character selection.
- **Characters exist only in the host's world** (owner's decision, 2026-11-15). Creation happens
  inside a session: every creation step (race, gender, stat, skill, accept) is an action the host
  applies with the shared rules (`arcanum_rules`), exactly like level-up. The host never receives
  a finished character sheet to validate, so its ledger is correct by construction. The client
  runs the same rules only to preview a change and grey out buttons; the host's result counts.
- **New Game starts the session first:** New Game starts the (in-process) host and its session,
  then character creation or the pre-made picker runs in it. The pre-made picker's choice is an
  action too: the host builds the character from the PC prototype. Once the character is
  accepted, the host writes the initial save (character, starting map and position) and the world
  boots from it.
- Combat is turn-based only. This milestone has no combat, but the message design must not assume
  a real-time mode.
- From earlier discussion: the original streams 64×64-tile sectors and records per-sector
  changes. Streaming follows that model, not per-entity hydration radii.

## Approaches considered

1. **Single-player world first, add the network split later.** Fastest to see a world. Rejected:
   every system gets rewritten when authority moves to the host.
2. **Netcode first, with an empty world.** Proves the transport early. Rejected: with no state to
   carry, the message set is guesswork and gets redone.
3. **The world built on the split from its first line (chosen).** It is slightly slower to the
   first picture, but every message carries real state, and A2 tests the architecture before any
   content is built on it.

## Build order

Each step is its own PR and ends in something testable.

1. **Map formats** (`arcanum_formats`). Readers for sector files (tiles, blocking, the object
   list), the object types needed for static scenery (walls, portals, scenery) in addition to
   PCs, map properties, the tile-name tables, and art id → path via the
   `.mes` index files. Find the map and position the original starts a new game on (Arcanum.exe
   and the data). Done when every sector of the starting map parses in a real-data test.
2. **`arcanum_sim` and `arcanum_net` crates.** `PlayerIntent`, `LedgerDelta` stamped with a
   tick number that only increases, a `Transport` trait, and `LoopbackTransport`. Done when a
   headless test sends an intent and gets the expected delta back.
3. **Save format and boot.** A `WorldSave` holding the session's state (players' characters, map,
   positions, and later the per-sector changes). New Game starts the host session; the
   character is created or picked in it; on accept the host writes the initial save with the
   starting map and boots the world from it. A development
   quick-save key serializes the live host state (the inn/campfire rule is off in development).
   Done when tests round-trip a save, including one taken mid-session, and boot an identical sim
   from it.
4. **Host sim loads the world.** The starting map's sectors around the PC, the PC as an entity,
   and movement validation at a fixed tick. Tile pathfinding and movement rules are shared code
   in `arcanum_sim` (plain functions, usable from both worlds) over the static blocking data plus
   host-replicated dynamic blockers. Done when headless tests give the right path, accept a valid
   client move, and correct an invalid one.
5. **Client draws the world.** Isometric terrain, tiles and static scenery (depth-sorted) for the
   loaded sectors, and the camera, replacing the placeholder. Unchanged map content comes from
   the client's own copy of the original data; game state and changes to map content come only
   from host deltas. Done with a screenshot check on real data.
6. **`.art` animation playback.** Frame timing, 8 directions, frame offsets. The PC stands and
   walks.
7. **Click to move, end to end.** Click → tile → the client paths and walks its PC at once →
   sends the move to the host → the host validates and accepts or corrects → the client smooths
   any correction. **Milestone A.**
8. **Milestone A2.** `UdpTransport`, a second client process, and a join: the joiner sends a
   player id; if the host's save has no character for it, the joiner creates or picks one through
   creation actions the host applies (no finished sheet is ever submitted); the host adds it to
   its `WorldSave` and sends the snapshot, then deltas. Two PCs on the map, each controlled from its own window.

## Step 2 as built

The crates follow step 2, with these differences:

- **Two transport traits, not one.** `HostTransport` receives every player's intents (tagged
  with the `PlayerId`) and sends a delta to one player; `ClientTransport` sends its own player's
  intents and receives deltas. With one trait both ends could call every method. `LoopbackHost`
  hands out a `LoopbackClient` per player through `connect`, so one host serves several clients.
- **Deltas are not yet stamped with a tick.** `LedgerDelta::Tick(n)` marks the end of a host tick
  instead. Stamping every delta (an envelope with the tick, for `last_confirmed_tick`) is still
  owed and fits best with `UdpTransport` in step 8, where ordering and duplicates arise.
- **"Done when" was half met** in the step 2 PR (intents arrived; nothing answered). The
  session below completes it.
- **No snapshot delta.** `WorldSave` arrives in step 3; the snapshot variant comes with it.
  `Spawn` carries an `EntityKind` and a position.
- **No separate pre-made intent.** `CharacterAction::PickPremade` already covers it.
- **`SimApp`** is a headless `App` with `TimePlugin` and `SimPlugin` only, ticking
  `FixedUpdate` at 30 Hz (`TICK_HZ`). Its owner calls `update`.
- **`arcanum_rules` has an optional `serde` feature** for the sheet and actions, as
  `arcanum_formats` has for `TileLoc`. `arcanum_net` has no serde yet; the loopback passes
  values, not bytes.

## Character creation through the host (first part of step 3)

The in-process host now owns the character; the screens are unchanged.

- **Shared creation code.** `arcanum_sim::Creation` (sheet plus the picked pre-made) is what the
  client's `CharacterHost` used to hold. Host and client both run it, the host to decide and
  the client to preview.
- **Numbered requests.** `PlayerIntent::Character { seq, request }` carries `New`, an `Action`
  or `Accept`. The host answers every one with `CharacterUpdate { ack, creation }`, refused or
  not, and an accepted character with `CharacterAccepted`. The client shows its own preview at
  once; on an answer it takes the host's copy and replays the requests numbered after `ack`.
  Refusals are found by the preview, so they are not sent.
- **Session.** The host's `Session` holds each player as `Creating(Creation)` or
  `Ready(CharacterBlueprint)`. A `Ready` player can only send `New`. `InWorld` waits for the PC
  entity (step 4).
- **No transport in `arcanum_sim`.** The host reads an `Inbox` and writes an `Outbox`;
  `arcanum_net::LocalHost` moves them to and from a loopback transport. The client keeps the
  `LocalHost` as a non-send resource (it holds a Bevy `App`) and updates it in `PostUpdate`
  (`sim_bridge.rs`).

Not done, or done differently from the plan:

- **The player id is fixed** (`LOCAL_PLAYER`, 1). The random id saved in the player's config
  comes with joining (step 8).

Resolved in the next PR (the save): loading waits for the host, and the host is created on
entering New Game.

## The initial save (second part of step 3)

- **`WorldSave`** (`arcanum_sim::save`): format version, tick, map name, each player's
  `CharacterBlueprint` and position, and per-sector changes (`SectorDelta`, empty for now).
  Encoded as JSON in one place (`to_bytes`/`from_bytes`); a different version is refused.
- **On accept** the host puts the character into its `WorldState` at the start position. The
  first accepted character starts the world on the start map (`StartMap`, the `START_MAP` entry
  of `Rules/MapList.mes`: `Arcanum1-024-fixed` at 92958,82592). The save is written at the end of
  any tick that changed it, to `<config dir>/open-arcanum/saves/<session>.save`, where the
  session is the host's start time in milliseconds. Without a save folder (tests) nothing is
  written.
- **Loading waits for the host.** The editor's and the picker's `LoadRequest`s use
  `after_host_accepts`: the host's answer counts as one more item to load. The host sends
  `CharacterAccepted` before the update that answers the `Accept`, so an answer without it is a
  refusal; loading then returns to the main menu, as it does when no host is running.
- **The host lives with the new game.** It is created on entering the New Game choice menu, the
  pre-made picker or character creation (whichever comes first), with the rules and start map
  loaded then, and dropped on going back to the main or Single Player menu. A new host comes
  with a fresh `CharacterHost`. The host still takes the game's rules again if `CreationData`
  changes after it started.

Not done, or done differently from the plan:

- **Nothing boots from the save yet,** and there is no quick-save key. Step 3's "boot an
  identical sim from it" and the mid-session save come with the host loading the map (step 4),
  when there is state beyond the characters.
- **Load Game does not list these saves yet.**
- **A save is written per session,** named by start time, not chosen by the player.

## The host loads the map (first part of step 4)

- **Booting from the save.** `boot_world` (`arcanum_sim::world`) keeps the host's world in step
  with its `WorldState`: it opens the save's map (`map.prp` into `LoadedMap`), spawns a PC for
  each player in the save who has none (`NetId`, `PlayerControlled`, `Position`, `Sheet`), and
  loads the sectors within one sector of it (a 3 x 3 block, fewer at the map's edge). A new
  game and `SimApp::boot(save)` take the same path, so two hosts booted from one save have the
  same PCs with the same net ids. The player's session state becomes `InWorld(NetId)`.
- **Spawns.** Each new PC is announced with `Spawn` to every player in the session; its player
  also gets a `Spawn` for each PC already there. The client keeps its own PC's spawn in
  `LocalPc`.
- **Loading waits for the world.** `LoadRequest::after_world_boots` (was `after_host_accepts`)
  waits for acceptance and for the local PC's spawn: two items on the progress count.
- **Missing and broken files.** A sector without a file is recorded as absent; a file that
  does not parse is recorded in `LoadedMap::errors` and treated as absent. On the real start
  map all nine sectors around the start load without errors.

Not done, or done differently from the plan:

- **Tile masks only.** `LoadedMap::tile_mask_blocked` reads the sectors' tile blocking masks;
  object blocking is not decoded (`TODO.md` → World).
- **No streaming:** sectors load when a PC spawns and are never unloaded (`TODO.md` → World).
- **The save is not updated from the entities.** Positions in the save are the start
  positions; once PCs move (step 4's movement), the save must be written from the entities.
- **Pathfinding and movement validation** are the rest of step 4, below.

## Pathfinding and movement validation (rest of step 4)

- **Pathfinding** (`arcanum_sim::pathfinding`): `find_path` is A* over the tile grid, plain
  functions over a `Walkable` map so the client can use them too. Steps go to the 8 neighbours,
  costed 10 straight and 14 diagonal; a diagonal may not cut past an unwalkable tile. Ties break
  the same way every time. The search gives up after `MAX_SEARCH` (20 000) tiles. `LoadedMap`
  is `Walkable` where the sector is loaded, the tile is inside `map.prp`'s size, and the tile
  mask does not block.
- **Movement follows the hybrid-trust design, not "the host paths".** The client sends
  `PlayerIntent::Move { seq, path }` (was `MoveTo(goal)`) for the path it is already walking.
  `check_path` accepts it when it starts on the PC's tile or one step ahead (the client walks at
  once), every step is allowed by the same `step_allowed` A* uses, and it has at most
  `MAX_PATH_STEPS` (256) tiles. The host then walks the PC itself (`Walking`) at
  `WALK_TILES_PER_SECOND`, so a client cannot outrun it: one that runs ahead sends a path that
  starts too far away and is corrected. Accepted moves go to the other players as
  `Move { net_id, path }`; refused ones get `MoveCorrection { net_id, seq, position }` (`seq`
  added) and the PC stops on the host's tile.
- **Every step is checked again as it is taken.** `walk` checks each step against the map just
  before taking it, since doors close while a path is walked. The host never plans a PC's path
  (owner's call: the client plans, the host validates). On a blocked step the PC stops and
  every player gets a `MoveCorrection`; the client replans from there.
- **Actors block only in combat** (owner's call). Out of combat, PCs and NPCs do not block PCs:
  they walk through each other and may share a tile, so a client is never snapped back because
  another actor moved. In combat actors block (`Occupied`, which the corner rule counts too) and
  the snap-back is enforced. Combat start must move apart actors sharing a tile, in the
  combat-start pause.
- **Clients follow the host's state as it arrives** (owner's call): other actors are drawn from
  the deltas, and the drift that brings is accepted.
- **The save follows the PCs.** A PC that stops (end of path or correction) has its position
  put into the `WorldState`, which writes the save.
- **Real data:** from the start tile, every tile within 20 that A* reaches gets a path the host
  accepts (1680 tiles).

Not done, or done differently from the plan:

- **The tile mask blocks nothing around the start.** None of the 41 x 41 tiles round the start
  position is mask-blocked, so until object blocking is decoded the host lets a PC walk through
  the wreckage and scenery there (`TODO.md` → World).
- **Walking speed** was a placeholder (3.5 tiles/s); step 6 replaced it with the walk art's
  timing (below). The 10/14 costs and the no-corner-cutting rule are not checked against the
  original.
- **No combat yet,** so nothing blocks but the map: `Occupied` is written and unused.
- **Corrections, for the client (step 7) and remote play:** the client slides to a corrected
  tile rather than jumping, and ignores a correction whose `seq` is older than its last `Move`
  (the answer to the newer one decides). A new path must start on the host's tile or one step
  from it; with real latency the host walks further before a new path arrives, so before UDP
  the tolerance must cover the tiles walked since the client's last acknowledged state.
- **NPCs give way to PCs (owner's rule, for when NPCs move).** PCs do not block for each other
  out of combat, but they do for NPCs: when an NPC's next step is blocked by a PC, the NPC stops all movement for 5 s (150 ticks), then plans a new route from
  where it stands. PCs keep replanning at once. The pause is not built: there are no NPCs yet.
- **NPCs step out of a shared tile (owner's rule).** Out of combat a PC may end up on an NPC's
  tile. Once nothing on that tile is moving, the NPC walks to the nearest tile no actor stands
  on. The host plans that step, since it owns NPCs.
- **No tether and no combat rules** yet: the check is walkable tiles and speed only.
- **A move is not stamped with a tick,** and the relayed `Move` has no `seq`.

## The client draws the ground (first part of step 5)

- **Map name from the host.** `LedgerDelta::EnterMap { map }` comes before the player's PC
  `Spawn`; the client keeps it in `LocalMap`.
- **The client reads the map itself.** `world_render::ClientMap` holds a `LoadedMap` (the
  host's reader) filled from the client's own VFS around `LocalPc`, and the art index.
- **Ground tiles** are sprites at the original's tile centres (`isometric::tile_to_world`, from
  `loc_to_xy` 0x4b8680; see `NOTES.md` → "Tiles on screen"). Each sits in the fixed 78 x 40
  rectangle with no hotspot, mirrored by the original's rule, with the facade frame from the
  tile id. World units are original pixels, relative to the corner of the PC's first sector so
  `f32` stays exact. The camera shows 600 units vertically, the original's screen, centred on
  the PC. World sprites use nearest sampling: linear filtering showed a line round every tile.
- **Debug start:** `DEBUG_ARCANUM_SCREEN=ingame` makes a character named "Debug" and goes
  through loading into the world; with `DEBUG_ARCANUM_SCREENSHOT` the capture waits until every
  tile has its art. Checked by eye on the real start map: seamless ground and the crash site.

Not done, or done differently from the plan:

- **Walls, portals and scenery are not drawn** (the implementation plan's PR 6 had them). The
  crash wreckage shows because it is facade tiles. Objects come next, as the plan's split
  suggested; the placement rule is in `NOTES.md`, the draw order is not traced yet.
- **No PC sprite** (critter art ids are not decoded; animation is step 6).
- **Sectors stream in only.** The client reads sectors as the PC's position changes, but never
  unloads them, and draws all 4096 tiles of each (no culling beyond Bevy's).
- **The camera snaps to the PC;** following with a lerp waits for movement (step 7).
- **The loading screen does not wait for tile art,** so tiles can appear a frame or two after
  the world shows.
- **Tile draw order** is not reproduced (all tiles at the same depth): ground tiles do not
  overlap, so it shows nowhere yet.

## The client draws walls, portals and scenery (rest of step 5)

- Each wall, portal and scenery object of a loaded sector is a sprite. It sits at the tile
  centre plus its offset fields, anchored on its frame's hotspot.
- The rotation and frame come from the art id, with the original's wall and portal hotspot and
  mirroring rules (`NOTES.md` → "Objects on screen").
- Depth follows the original's classes and scan order. Flat objects draw first, then the others
  by screen row, place along the row, and order on the tile. It is packed into an exact integer depth, and the
  camera's depth range is widened to hold it.
- Hidden objects (flags 0x1, 0x2, 0x100) are skipped.
- `ArtFrames` now loads every rotation.
- `DEBUG_ARCANUM_START=x,y` starts new games elsewhere on the start map.
- Checked by eye at the start and in two Shrouded Hills houses: walls join up in every rotation,
  and furniture, doors and rugs draw in the right order.

Not done:

- **No wall cutaway.** Front walls draw at full height; the original hides some near the PC.
- **No roofs, shadows, overlays, light or blend effects.** `blit_flags`, `blit_scale` and
  flag 0x80 are ignored, so the wreck's fires draw opaque.
- **Objects without their own `current_aid`** (about 3% of scenery) are left out. The
  original takes the prototype's art; wall, portal and scenery prototypes are not read yet.
- **No animation:** objects show the frame their art id names.
- **Depth reach.** Objects on one tile keep the sector's list order, which matters for walls
  and the furniture against them (the owner spotted broken corners, shelves, a bed and a candle
  stand in the house at 90330,84122). Only 8 per tile are kept apart; more tie. Order holds
  within about 256 tiles of the first sector's corner, which needs a re-origin once sectors
  stream far.

## Animation: the PC stands and walks (step 6)

- **Critter art ids** (`arcanum_formats::art_id::CritterArt`, from Arcanum.exe: builder
  0x503c00, path sprintf 0x41dd33, normaliser 0x41d510). Gender, art body, armour, frame,
  rotation, animation, palette and weapon fields; `art/critter/<body><g>/<body><g><armour>
  <x|s><weapon><anim>.art`. Elf women use the human woman's body (no `eff` art exists), plate
  armour is male with fixed bodies. Real data: all 390 distinct critter ids in the `.mob` NPCs
  and PC prototypes, and their walk animations, name files that exist.
- **The PC's art** (`arcanum_sim::pc_art`): the race's art body (table 0x5c5170), the gender,
  underwear, standing, rotation 4, as the new-PC prototype (0x40d952) and creation (0x441c70)
  build it; it matches all 13 PC prototypes. `Spawn` now carries the art id and the speed stat.
- **Walking speed (owner's goal: match the original).** The original moves a walker by its walk
  art's frame deltas, so speed depends on body and direction (a human's loop covers about
  47 px diagonally, a dwarf's 30, a half-ogre's 61). Frames per second come from the speed stat
  (0x437990: walk 6/17/30 at speed 0/8/30, +4 for dwarf, gnome and halfling bodies), frame time
  `clamp(1000 / fps, 30, 800)` ms (0x42ee90). `arcanum_sim::Gait` reads the walk art's deltas
  and times each step: its screen length over the pixels per frame in its direction. A human
  at speed 8 takes 0.55 s for an edge step and 0.77 s across (was a flat 3.5 tiles/s). Host
  and client use the same `Gait`.
- **Client** (`actors.rs`, `animation.rs`): each `Spawn`ed PC is a sprite playing its stand
  art, or its walk art at the gait's frame time, in the step's direction (rotations clockwise
  from screen-up). It slides between tile centres; depth puts it after the objects on the lower
  of its two tiles. Other players' PCs follow `Move`; `MoveCorrection` puts a PC on the host's
  tile. The camera follows the PC's sprite.
- **Local walking ahead of step 7:** `WalkLocalPc { goal }` plans with `find_path` on the
  client's map, walks at once and sends the `Move`. `DEBUG_ARCANUM_WALK=x,y` sets it on entering
  the world. Checked by eye on the start map: the PC stands, walks up-right with the camera,
  and stops.

Not done, or done differently from the original:

- **Tile crossing is not traced.** The original adds frame deltas to the offset and re-bases
  at some point (found only for the knockback mover, 0x42f390); here a step is one tile's screen
  distance and the sprite slides linearly, so feet can slip slightly within a step.
- **The speed curve between 0, 8 and 30 is inferred** (linear joins).
- **The normaliser's animation fallbacks** (table built at 0x41ceed: a body without an
  animation gets another) are not reproduced; stand and walk exist for every body.
- **The 5-rotation mirror** (stored rotations 0, 4-7, the rest drawn flipped; flag 0x5bea14)
  is not used: the data stores all 8 rotations.
- **No running** (anim 6) and no NPCs.

## Builder's calls (proposed by Claude, not owner decisions)

- `arcanum_sim` uses `bevy_ecs` (and `bevy_app` for scheduling) with no rendering, audio or
  windowing. Host systems are ordinary Bevy systems on a fixed tick. In single player, the host
  runs in the same process as the client.
- `arcanum_sim` and `arcanum_net` do not depend on `arcanum_client`. `arcanum_net` depends on
  `arcanum_sim` for message payload types.
- Saves are this project's own format (serde), not the original's. They store references into the
  original data (prototype numbers, map names) plus deviations, never copies of original assets.
- The join snapshot is a `WorldSave`. One serializer serves saving, loading and joining.
- The client keeps `last_confirmed_tick` and drops stale or duplicate deltas, as in
  `multiplayer.md`.
- Entities are identified across the wire by host-issued ids, never by Bevy `Entity`.
- Until Steam ids exist, a player is identified by a random id stored in that player's config.
  The host keys session characters by it.
- Static scenery blocks movement on the host from the same data the client draws.

## Left to the builder

- Exact message variants, beyond the move report, the correction, and position, spawn and
  despawn deltas.
- Validation tolerances and how corrections are smoothed.
- The save file's location and name, and how `SaveGames` lists it.
- Pathfinding algorithm details, movement speed per tick, how many sectors are loaded around the
  PC.
- The UDP framing and the wire encoding.

## Related

- [Multiplayer](multiplayer.md): the co-op architecture this milestone starts.
- [Combat design](combat_design.md), [Companions](companions.md): later milestones.
