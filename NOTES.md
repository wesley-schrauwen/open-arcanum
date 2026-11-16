# Project notes (handoff)

Working notes for whoever picks this up next, human or agent. Read this first, then `README.md`.
Last updated: 2026-11-15 (character creation and the stats page).

## Handoff: start here

- **Branch / PR stack.** Every change is an open PR stacked on the previous one, oldest first:
  #9 (interactive menu, base `main`) → #10 → #11 → #12 → #13 → #14 → #15 → #16 → #17 → #18 → #19
  (world milestone, another session's map/sector/object readers) → #20 (character creation) →
  #21 (fonts) → #22 (name text field) → #23 (Play button, Multiplayer entry) → #24 (stats page,
  branch `feat/stat-screen`, the newest; `git log` there has everything). Merge in order and
  **retarget each PR to `main` before merging** (stacked PRs merged into their parent branch do
  not reach `main`; this already bit once, fixed by #8).
- **Work tree** is clean on `feat/stat-screen` at handoff; all branches are pushed.
- **Not yet checked by eye:** the stats page (#24). Screenshots came out fully black late in the
  session for every screen, even ones that worked the day before (display asleep/locked, not a
  code change). Run `DEBUG_ARCANUM_SCREEN=charactereditor DEBUG_ARCANUM_SCREENSHOT=/tmp/x.png
  cargo run -p arcanum_client` with the display awake and check the layout against the
  original coordinates in "Character editor" below.
- **Also unverified with a real mouse:** clicking outside the name field to unfocus it (tests use
  Enter), and the picker/creation arrows (fixed a full-screen layer swallowing clicks; owner to
  confirm in-game).
- **Reverse engineering workflow (owner's preference):** send REA / disassembly questions to
  Sonnet sub-agents, batched by topic, with the known addresses as context; verify their numbers
  against the game data before relying on them (several reports were cross-checked this way).
  `objdump -d --start-address=... --stop-address=...` on `assets/Arcanum/Arcanum.exe` works for
  code Ghidra did not mark as functions. The original menu screens are a window table at
  0x5c3a40 (records: background art, callbacks, button lists); see the sections below.
- **Open questions for the owner:** "Play" vs "PLAY" on the main menu (the font's small capitals
  make "Play" look small; screenshots compared, left as "Play"); the pocket-watch replacement
  animation (owner will supply a GIF/SVG, see `TODO.md`).


## What this is

A reimplementation of *Arcanum: Of Steamworks and Magick Obscura* (Troika, 2001; originally C++)
in Rust on **Bevy 0.20**. The client reads the original game data from the player's install.
The finished game is planned to ship upscaled 4K art, resampled audio, updated animations and
increasingly original art. Players will still need a valid Steam copy. Code is MIT; game assets
are not.

## Current state

Working end to end: `cargo run -p arcanum_client` opens a window and shows the original main menu
at any supported resolution, with the original menu music, sounds and fonts. Play → New Game →
Pick Character (pre-made picker) or New Character (creation, then the stats page) → loading
screen → world placeholder. Load Game shows the original save screen (no saves exist yet).
Options opens the original options screen, settings persist in `config.toml`. Credits and the
world are placeholders. Esc goes back one level (not from the loading screen or while typing in
a text field).

| Area | State |
|---|---|
| `.dat` archives | Done. Reader, layered VFS, override folder. |
| `.mes` text | Done. Decoder + Bevy asset. |
| `.art` sprites | Frame decode done (raw + RLE, palettes, 8 rotations). No animation playback. |
| Bevy integration | Done. VFS is Bevy's default asset source; `.art` → `Image`, `.mes` → `MesAsset`. |
| Main menu | Play / Options / Credits / Exit Game ("Play" is our label for the original's Single Player). Play menu: New Game, Load Game, Continue (dimmed), Multiplayer (dimmed), Cancel. Each menu's last entry sits at the bottom of the panel. Credits and the world are placeholders. |
| New Game flow | Choice menu (Pick Character / New Character / Cancel, `MainMenu.mes` 420-422) → pre-made picker (12 characters from their prototypes: portrait, name, gender, race, background title, story) → loading; or → character creation (name text field, portrait/gender/race/background arrows, initial stats, race description or background story) → stats page (spend 5 points on stats, hit points, fatigue; derived stats and resistances) → loading. See the sections below. |
| Load Game screen | Original art and layout from Arcanum.exe's screen table (0x5c4560): top bar, `SaveLoadBackground`, bottom bar; Back/Load round buttons, Delete, info toggle, hover help in the bottom bar. Lists `SaveGames` (empty until saving exists; `DEBUG_ARCANUM_SCREEN=loadgame` uses example saves). Load goes through the loading screen to the world placeholder. No portraits/screenshots or delete confirmation yet. |
| Loading screen | Scaffolded: `LoadRequest { next, assets, min_secs }` + `AppState::Loading`; waits for assets (failures warn and continue) and at least 1.5 s; `LoadingProgress` for a future bar. Shown entering the game only (owner's decision). |
| Options screen | Original art and text; Game/Video/Sound tabs, arrows on every setting, commit on change. Resolution: arrows pick, Done applies, then "Confirm Resolution" with a 15 s auto-revert. Saved to `config.toml` on leaving the screen. Resolution, fullscreen, brightness and the three volumes take effect; the rest are stored only. |
| Brightness | A gamma curve over the finished frame (`brightness.rs` + `brightness.wgsl`, Bevy's `FullscreenMaterial` scheduled after `ui_pass`, before `upscaling`), so menus and world both change; colour grading would skip the UI. 0.5 is neutral and pixel-identical; exponent 2^±1 at the ends; black and white stay put. Live. |
| Audio | Menu music (`sound/music/Arcanum.mp3`, loop, 50% × Music setting) and hover/click sounds (× Effects). Options arrows use `Button_Medium.wav` on press and `Button_Medium_Release.wav` on release, no hover sound, as in the original (confirmed in Arcanum.exe at 0x57fe60: art 667/668 with sound ids 3000/3001); text buttons keep `MorphText_CLICK.wav`. Voice volume applies to anything spawned with `play_voice`/`VoiceLine` (no voice plays yet). All live. |
| Fonts | Game bitmap fonts decoded and rendered (`BitmapText`, multi-line: wrap, `\n`, tab indents, whole-line clipping). Menu uses `Morph30Font`/`morph15font`; character screens add `Flare12Font` (`UiFont::Body`) with the original white/red tints. Override TTF still wins if present; built-in font as last resort. |
| Game data formats | `.mes`, `.art`, fonts, `.dat`; objects (`arcanum_formats::object`: prototypes and instances, PC field tables from the engine's names); maps/sectors (#19). |
| Resolutions | Done. 720p–8K, 4:3–2.4:1, `UiScale` from a 1080 baseline. |
| Tests | 278 tests, 254 need no game data (synthetic archives/art/fonts, headless Bevy). |
| Everything else | Not started: game screens, maps, critters, dialogue, scripts, saves. |

### Git state

`main` is current. On 2026-11-13 the stacked PRs #2–#7 were found to have merged only into their
base branches; PR #8 merged `test/review-and-headless-tests` into `main` to bring everything in.
Branch new work from `main`. When stacking PRs, merge bottom-up and retarget each PR to `main`
before merging it (or delete the merged base branch so GitHub retargets automatically).
The old `feat/*` branches are fully merged and can be deleted.

The GitHub repo was renamed to `wesley-schrauwen/open-arcanum`. The local `origin` still points at
`arcanum-2027` and works through GitHub's redirect.

## Layout and key files

```
Cargo.toml                       workspace (edition 2024); root src/main.rs is an unused stub
crates/arcanum_vfs/              game data access, no Bevy dependency
  src/archive.rs                 ArcanumArchive: one .dat file (format notes at top of file)
  src/vfs.rs                     Vfs: override dir + archives, case-insensitive, path-escape safe
  src/test_util.rs               DatBuilder: writes synthetic .dat files (feature "test-util")
crates/arcanum_formats/          file format decoders, no Bevy dependency
  src/mes.rs                     MesFile: {id}{text} message files
  src/object.rs                  Object (prototype/instance, all 18 types), PlayerFile (.mpc), type numbers
  src/sector.rs                  Sector (.sec): lights, tiles, roofs, scripts, blocking mask, objects
  src/object_fields.rs           field tables per block (generated: tools/object_probe/gen_rust_fields.py)
  src/cp1252.rs                  Windows-1252 decode/encode (shared by .mes and fonts)
  src/font.rs                    BitmapFont: glyph lookup, measure, render to a coverage mask
  src/art.rs                     ArtFile: sprites (format notes at top of file)
  src/test_util.rs               ArtBuilder, TestFrame, rle_encode, font_art, ObjectBuilder,
                                 SectorBuilder (feature "test-util")
  tests/real_files.rs            parses every .mes/.art in the real archives (skips without data)
crates/arcanum_rules/            character rules, no Bevy (shared by client and future host)
  src/sheet.rs                   CharacterSheet (accepted values + Draft), Refusal, Message
  src/data.rs                    RulesData: backgrounds, effects, portraits, tables, pre-mades
  src/ops.rs                     operations and queries (raise/lower, accept, derived, ...)
  src/tables.rs                  skill, tech, spell and schematic tables
  src/action.rs                  CharacterAction + RulesData::apply
  src/blueprint.rs               CharacterBlueprint, pre-made sheets, InvenSource parsing
crates/arcanum_client/           the Bevy game
  src/main.rs                    app setup; DEBUG_ARCANUM_SCREENSHOT handling
  src/resource_pack.rs           ResourcePack (game dir, override, module) + VFS mount order + `paths::*`
  src/vfs_asset.rs               VfsAssetReader / VfsAssetPlugin: VFS as Bevy's asset source
  src/art_loader.rs              ArtAssetLoader: .art → Image (frame 0); ArtFramesLoader: all frames
  src/mes_loader.rs              MesAsset / MesAssetLoader
  src/settings.rs                DisplayResolution, GameSettings (serde, sanitize), EnvOverrides, DisplayPlugin, UiScale
  src/config.rs                  config.toml: path, lenient load, atomic save on leaving Options
  src/main_menu.rs               AppState, MenuAction, MainMenuPlugin: camera, background, buttons,
                                 hover highlight, Activate observer, menu_ready
  src/options.rs                 OptionsPlugin: Options screen, OptionId/OptionsTab model, resolution confirm
  src/widgets.rs                 WidgetsPlugin: HoverText (+Selected), FrameButton (art-frame buttons)
  src/screens.rs                 ScreensPlugin: placeholder screens (world, Credits), Esc back to the menu
  src/brightness.rs             BrightnessPlugin: gamma pass after the UI (brightness.wgsl)
  src/menu_window.rs             MenuWindowPlugin: shared 800x600 frame, Back/Next, hover help
  src/new_game.rs                NewGamePlugin: pre-made character picker
  src/premade.rs                 pre-made characters from their prototypes (name, race, story, portrait)
  src/load_game.rs               LoadGamePlugin: Load Game screen, SaveGames, LoadGameState
  src/loading.rs                 LoadingPlugin: loading screen, LoadRequest, LoadingProgress
  src/audio.rs                   MenuAudioPlugin: menu music, MenuSounds (hover/click)
  src/fonts.rs                   FontsPlugin: ArcanumFonts, UiFont, insert_text (picks TTF/bitmap/default)
  src/bitmap_text.rs             BitmapTextPlugin: font asset + loader, BitmapText rasterise/tint/fallback
  tests/headless.rs              81 Bevy tests, no window/GPU/game data
  tests/vfs_assets.rs            real-data tests, incl. "every resource_pack path exists"
assets/                          symlink to the local game install; gitignored, never commit
plans/                           design plans for future work (combat_design.md: companion combat AI; companions.md: follower management; multiplayer.md: co-op architecture)
```

## Running

```sh
cargo run -p arcanum_client                         # needs game data
cargo test --workspace                              # real-data tests skip without game data
```

| Env var | Purpose |
|---|---|
| `ARCANUM_DIR` | Game folder with `arcanum1.dat` etc. Default `assets/Arcanum`. Tests honour it too. |
| `ARCANUM_OVERRIDE` | Loose-file folder that shadows everything else. |
| `ARCANUM_MODULE` | Module to mount from `modules/`. Default `Arcanum`. |
| `ARCANUM_RES` | `WxH`, e.g. `3440x1440`. Overrides the saved resolution for this run only (not written back). Invalid values are ignored. |
| `ARCANUM_FULLSCREEN` | `1`/`true` for borderless fullscreen. Overrides the saved value for this run only. |
| `ARCANUM_CONFIG` | Path of the settings file to use instead of `<user config dir>/open-arcanum/config.toml` (macOS: `~/Library/Application Support/open-arcanum/`). |
| `DEBUG_ARCANUM_SCREENSHOT` | Path; saves the window once the menu (or Options) is ready, then exits. Used to verify rendering. |
| `DEBUG_ARCANUM_SCREEN` | `options`, `singleplayer`, `newgamechoice`, `newgame`, `createcharacter`, `charactereditor`, `loadgame` (with example saves) or `loading` (held for 60 s) opens straight onto that screen (for screenshots). Screenshots are taken 60 frames after the screen is ready; earlier captures came out black. |

## Format knowledge

Recovered by inspecting data and by reverse engineering `Arcanum.exe` with the REA MCP tools
(Ghidra). Full layouts are in the doc comments of `archive.rs` and `art.rs`.

- **VFS mount order** (`ResourcePack::open_vfs`, highest priority first): override folder,
  `modules/<module>.PATCH*` (highest number first), `modules/<module>/` (loose files: music,
  maps, saves), `modules/<module>.dat`, then every `.dat` in the game folder. Music is only in
  the module folder. `modules/Vormantown.dat` is a second module.
- **`.dat`**: index at the end of the file. The last 28 bytes are a footer: GUID, `1TAD`, unused u32,
  index+footer size. Entry flags: `0x1` raw, `0x2` zlib, `0x400` directory. Paths use `\`.
  Archives in the game folder: `arcanum1-3.dat`, `Arcanum4.dat` (patch), `tig.dat`. Higher
  numbers shadow lower ones; the VFS mounts them in descending name order.
- **`.mes`**: `{id}{text}` pairs, Windows-1252, `//` comments outside braces, multi-line text.
  Braces do not nest. Pairs are read strictly in order.
- **`.art`**: loader is `FUN_0051b710` (called from `FUN_0051b170`, near the "ART LOAD FAILURE"
  string at `0x5bec2c`). 0x84-byte header, up to 4 BGR palettes, per-rotation frame tables
  (28 bytes: w, h, size, center x/y, delta x/y), then pixels. Raw if size == w*h, else RLE
  (bit 7 set = literal run, clear = repeat). Flag bit 0 = static (1 rotation), otherwise 8.
- **Unverified assumptions**: palette index 0 = transparent (decoder sets alpha 0); frame 0 uses
  the first palette present (the game picks a palette per art id). Not yet checked against the
  engine's blit code.
- **Sound**: `sound/snd_*.mes` map sound ids to files; `sound/schemeindex.mes` and
  `schemelist.mes` define music schemes. "M.Main Menu" (#2600) = `music\Arcanum.mp3 /loop /vol:50`.
  Menu text sounds are ids 3026/3027 (`MorphText_HOVER.wav`/`MorphText_CLICK.wav`). WAVs are
  16-bit PCM; music is MP3 (the `bevy` dependency enables the `mp3` and `wav` features).
- **Fonts**: no TTFs in the game. 34 bitmap fonts: `art/interface/*Font.art` plus
  `art/morph15font.art` in `tig.dat`. Each is static `.art` with 225 frames: frame 0 = text
  cursor, frame n = Windows-1252 byte n+31 (frame 1 = space, last = 0xFF). All glyphs share the
  line height; pixel palette indices are a grey ramp = anti-aliasing coverage (alpha mask).
  Glyph width = advance (no kerning; e.g. `R` includes its tail). Fonts are listed in
  `art/interface/interface.mes` (Morph30Font = id 327, next to MainMenuBack = 329).
  The font data has no shading or outline. The original menus draw it in red with a dark
  drop shadow (from screenshots of the original Single Player menu: solid strokes about
  `rgb(96,28,20)`, hovered about `rgb(184,48,33)`). `BitmapText` adds a black shadow one font
  pixel down/right at 80% opacity. The original also looks slightly embossed (lighter stroke
  centres); that effect is not reproduced and its source in the engine is unknown.
  **Assumption**: the main menu uses `Morph30Font`. The menu code (`FUN_00546340`) draws with font
  handles at `0x64c210`/`0x64c218`/`0x64c0cc`/`0x64c228`, chosen by flags; where those are created
  was not traced.
- **Options screen** (original): full-screen `OptionsMenuBack.ART` (800x600): title plate,
  tab box (Game/Video/Sound/Done = `MainMenu.mes` 4000-4003), 2x4 grid of 213x56 slots
  (`OptionSlider.ART` tube, `OptionMultiChoice.ART` value window), arrow buttons
  `SldrButt_L/R_Arrow.ART` (frames: 0 normal, 1 pressed, 2 hover). Setting names in
  `mes/options_ui.mes`; values in `OptionsOffOn/OptionsWalk/OptionsFloats.mes`
  (`OptionsDifficulty`/`OptionsCombatMode` exist but are unused by design). The original had
  no Cancel: changes apply live. Layout constants are in `options.rs` (original pixels).
- **Sectors (`maps/<map>/<sector id>.sec`)** (verified: all 1680 `.sec` in `Arcanum.dat` parse
  to the exact byte; layout table in `sector.rs`). Loader at 0x4d1a40 (from the sector cache
  `FUN_004d0540`; name `"%s\%I64u.sec"` at 0x5b7e6c). Order: `u32 light_count` + 48-byte lights
  (0x4f72c0, 0x4ddd20) · 4096 `u32` tile art ids, row-major (0x4f7c30) · `u32 roof_empty_flag`, then
  256 `u32` roofs only if it is 0 (0x4f8020) · `u32 version` 0x00AA0000+0..4 (0x4d1eb1) · v>=1: `u32 n`
  + n x 24-byte tile scripts (0x4f65b0) · v>=2: 12-byte sector script header (0x4f6250) · v>=3:
  town map info, aptitude adjustment, light scheme (3 x u32), sound list (3 x u32, 0x4f8150) · v>=4:
  512-byte tile blocking mask (0x4f8250) · the objects back to back with **no lengths** · `u32
  object_count`. The engine (`FUN_004f1350`) reads the count from EOF-4 first, then walks that many
  objects field by field, then checks the repeated count. Versions in the data: v4 1473, v3 169,
  v0 18, v2 6, v1 2. Sectors hold only walls, portals, scenery and traps (Arcanum.dat: 89,013 /
  2,107 / 170,719 / 2,063); containers, items and NPCs are `G_<guid>.mob` files in the map folder,
  one instance object each (12,723). How the map loader assigns `.mob` files to sectors is not
  reversed. `Arcanum.PATCH0` contains no `.sec`.
  Tile `u32` (type 0): art type bits 28-31 (0 tile, 0xB facade), terrain 1 bits 22-27, terrain 2
  bits 16-21 (`art/tile/tilename.mes`), edge 12-15, variation 9-11, flags 6-8, flip bit 0
  (accessors 0x5036b0..0x503990, 0x504fd0). Facade ids (0xB) use other bits (0x504a60..0x504ad0);
  only the facade number is decoded (see "Art ids" below). **Unverified**: blocking-mask bit order (read as bit i%32 of word i/32 for tile i),
  light field meanings, sound list meaning, the tile flag bits.
- **Objects (instances), all 18 types** (verified: every object in all 1680 `.sec` and all
  12,723 `.mob` in `Arcanum.dat` parses to the exact byte; layout table in `object.rs`). Reader
  `FUN_00406600` (0x406600): `u32 version` 0x77 · prototype ObjectID (24) · then, for kind != -1,
  `FUN_00409aa0` (0x409aa0): ObjectID (24) · `u32 type` · `u16 count` · `W` × u32 bitmap ·
  values of set bits in bit order. Types (engine numbers): 0 wall, 1 portal, 2 container,
  3 scenery, 4 projectile, 5 weapon, 6 ammo, 7 armor, 8 gold, 9 food, 10 scroll, 11 key,
  12 key ring, 13 written, 14 generic, 15 pc, 16 npc, 17 trap. Blocks: common + own block
  (0-4, 17), common + item + subtype (5-14), common + critter + pc/npc (15, 16).
  **Bitmap block rule** (`FUN_0040c030` 0x40c030, ranges `FUN_0040cba0` 0x40cba0): common starts
  at bit 0; each next block at the next 32-bit boundary after the previous block's last field
  (common 36 fields -> 64; item 23 -> 96; critter 33 -> 128). Words W = word of the last field
  + 1: 3 for 0-4 and 17, 4 for items, 5 for critters. This corrects the earlier "grouped by 64".
  Storage kinds come from the engine's static field table (`FUN_0040a8a0`..0x40b8d5, read by
  `FUN_004e44f0` 0x4e44f0): kind 3 i32; 4 i64 (`u8 present` + 8); 12 ObjectID (`u8 present` +
  24); 11 string; 5-10, 13 sparse array (`FUN_004e78f0`; element size is stored, 4/8/12/24
  seen). Exceptions to the name-based rules: `scenery_whos_in_me`, `projectile_parent_weapon`,
  `item_parent`, `npc_leader`, `npc_combat_focus`, `npc_who_hit_me_last`,
  `npc_substitute_inventory` are object ids; `npc_standpoint_day/night` are i64. The tables are
  generated by `tools/object_probe/gen_rust_fields.py` from the field names and
  `tools/object_probe/ftypes.json` (kind per field index). Projectile (4) and pc (15) never
  occur in sector or `.mob` data; projectile is covered only by synthetic tests. Prototypes of
  types other than pc are not read yet (`UnsupportedType`).
- **Art ids → `.art` paths** (`crates/arcanum_formats/src/art_id.rs`; all rules, bit layouts and
  Arcanum.exe addresses are in its top doc comment). Verified over the 621 start-map sectors: 2.54 M
  tile ids, 28,064 walls, 1,588 portals, 117,385 scenery, 1,012 lights, 6,483 roofs all resolve to
  files that exist. Not covered: one scenery object with `current_aid` 0xFFFFFFFF (no art) and
  3,799 of 121,185 scenery objects with no instance `current_aid` (prototype fallback, see
  "Known issues"). Type = `id >> 28`
  (0x502700): 0 tile, 1 wall, 3 portal, 4 scenery, 9 light, 0xA roof, 0xB facade (sector "tiles"
  of type 0xB); critter 2, item 6, container 7, monster 0xC, unique npc 0xD, interface 5 and eye
  candy 0xE are not decoded. Index files (`ART_INDEX_FILES`): `art/tile/tilename.mes` (four lists
  by key range 0/100/200/300: outdoor/indoor x flippable/fixed), `art/wall/structure.mes`,
  `art/portal/portal.mes`, `art/scenery/scenery.mes` (key = category * 1000 + number),
  `art/facade/facadename.mes`, `art/roof/roofname.mes`, `art/light/light.mes`. Tile file name:
  `<n1><n2><edge letter><var letter>.art` or `<n>bse...`; **flipped tiles (bit 0) use the stored edge
  unchanged** although the engine accessor 0x503700 remaps it (remapping gives missing files; not
  traced why). Facade ids use bits 17..24 (+0x100 from bit 27) as the `facadename.mes` position.
  Portal bit 9 turns the `U` in `...U0.ART` into `D`. Open: facade sub-fields, item/critter paths.
- Not yet investigated: the engine's 5-rotation mirrored-sprite optimisation, item/critter/monster/container/interface art id paths, fonts
  (`art/morph15font.art` and similar), sound formats, `.dlg`, `scr/` scripts, maps (`terrain/`,
  `modules/`), save games.

## Conventions and decisions

- Asset paths live in `resource_pack::paths`, grouped by kind (`backgrounds`, `interface`,
  `portraits`, `textures`, `items`, `critters`, `scenery`, `text`, `sound`). Add new ones there,
  plus to `ALL_FILES`/`ALL_DIRS` so the real-data test checks they exist.
- **Owner's design decisions (Options):** turn-based combat only (no combat mode setting); a
  single difficulty (no difficulty setting); no Module, Violence Filter, Follower Skills,
  Auto Switch Weapons, Always Run or Control Followers settings; every change commits immediately,
  no Cancel. Resolution is the exception: its arrows only pick a pending value; Done (or Esc)
  applies it and asks "Confirm Resolution" before returning to the menu. Keep keeps it; Revert, Esc
  or 15 s without an answer restores the previous one; either way the menu follows. The rest
  of the screen is locked while the question is up. Settings are saved to `config.toml` when
  the player leaves Options (so only a confirmed or reverted resolution is ever saved); boot order
  is defaults → file → env vars. Brightness and the three volumes are live; other settings are stored only for now;
  English only (a future text manager may hot-swap strings). Tabs: Game = Auto Attack, Combat
  Taunts; Video = Brightness, Text Duration, Floats, Float Speed, Fullscreen, Resolution; Sound = Effects, Voice, Music.
- UI and camera are authored against a **1080-unit-tall baseline**. The camera is
  `FixedVertical { viewport_height: 1080 }`, and `UiScale` = window logical height / 1080.
  Original 800x600 art is scaled ×1.8 and pillarboxed.
- `WindowResolution::new` takes physical pixels in Bevy 0.20, so `UiScale` uses the logical
  height (correct on Retina).
- Bevy 0.20 APIs differ from older docs and examples (`Camera2d`, `Sprite`, `Text`/`Node`, Parley
  text, `MessageWriter`). Check `~/.cargo/registry/src/*/bevy_*-0.20.1` when unsure.
- Tests: prefer synthetic fixtures (`DatBuilder`, `ArtBuilder`, `tempfile`) and headless Bevy apps.
  Real-data tests must skip cleanly when the game data is absent.
- The VFS reads synchronously inside the async `AssetReader`. Acceptable for now.
- Bevy 0.20 has no `Interaction` component. Buttons are the headless `bevy::ui_widgets::Button`;
  `ActivateOnPress` makes a press send `Activate` (handled by an observer), and
  `bevy::picking::hover::Hovered` tracks hover. Tests insert `Hovered(true)` and trigger
  `Activate` directly.
- Screens are `AppState` states; their UI roots carry `DespawnOnExit(state)`. The camera,
  background and music persist across menu states.
- Headless tests need `StatesPlugin`, `init_asset::<Font>()` and `init_asset::<AudioSource>()`
  (normally provided by `DefaultPlugins`); see `menu_app` in `tests/headless.rs`.

## New Game picker: research and plan (in progress)

New Game first shows a text menu (`AppState::NewGameChoice`, built on the main menu panel):
Pick Character / New Character / Cancel, `MainMenu.mes` 420-422. Confirmed in Arcanum.exe:
window table at 0x5c3a40; record 0x5c4bb0 (index 11) has background 329, first id 420, count 3,
buttons at 0x5c4b20 whose codes are window-table indexes (13 = pre-made picker 0x5c5270, 12 =
character creation 0x5c4e28 with art 765, -2 = back). Single Player's buttons use codes 11, 7, 9.
New Character is dimmed until character creation exists.

Owner's decision: build it properly (option a), in three steps, each its own PR:
1. **Multi-line `BitmapText`**: done (wrapping, real/escaped `\n`, `\t` indents, left/centre).
   Checked in a real-game screenshot with Lugard Bloodstone's story in the description panel;
   the full story is taller than the panel, so step 3 needs clipping or scrolling there.
2. **Object file reader**: done for player characters. `arcanum_formats::object`:
   `Object::parse_prototype` / `parse_instance`, `PlayerFile::parse` (.mpc), field tables generated
   from the engine names; real-data tests parse all 13 PC prototypes and all 12 `.mpc` to the last
   byte and check names, backgrounds, story ids, portraits, race and gender. The VFS now mounts
   `<ARCANUM_DIR>/data` (loose `proto/`, `Players/`) between the module and the base archives.
   Instances of all 18 object types are read now (see "Format knowledge", objects).
3. **Fill the picker screen**: done except the stats. `premade.rs` loads the 12 pre-mades
   (prototypes 16087-16098) through the VFS; `new_game.rs` shows, as the original does (draw
   code 0x5458d0 + 0x5448e0, mode flag 0x64c458 = 0 for the picker): bar 1 the name (field
   0x10e), bar 2 `MainMenu.mes` 740+gender, bar 3 720+race, bar 4 the background title (first
   line of `gameback.mes`), "Initial Stats" (739) atop the upper-right panel, and the story body
   (after the first line) wrapped across the whole large panel 0x5c5070, whole lines only. Bar
   texts centre when they fit (0x542df0). Arrows `Big_Grn_L/R` cycle with wrap; Next stores
   `ChosenCharacter` (prototype number). Not yet: the stat values in the upper-right panel (loop
   at 0x544f2b, untraced), story scrolling, fonts/colours (font slots 0x64c210-0x64c21c,
   near-white; our text uses the small font and INFO_COLOR). Then character creation.

What is known so far:
- **Screen**: window table entry at Arcanum.exe `0x5c5270` (background art 766
  `PreGenCharacterBase`, 4 buttons at `0x5c51b0`; init `0x545870`, button handler `0x5459f0`,
  refresh `0x5458d0`). Buttons (13-dword records: x, y, art, ..., hover-help mes id last):
  Next (675,55 `NextBut`, 7003), Back (69,55 `BackBut`, 7002) in the bottom bar; previous/next
  character arrows at (43,118) and (244,118), art 283/284, no hover help. Coordinates of
  bottom-bar buttons are bar-relative; others are screen-relative (background starts at y=41).
- **Drawing** is shared with character creation (`0x5448e0`). Text rects (x, y, w, h; background
  pixels): `0x5c4ec0` (339,23,409,19) one line atop the upper-right panel; `0x5c4ed0`
  (46,183,228,19) bar 1 (name); `0x5c4ef0` (66,243,186,19) bar 2; `0x5c4f10` (66,293,186,19)
  bar 3; `0x5c4f30` (66,343,186,19) bar 4; `0x5c5060` (95,28,64,64) portrait; `0x5c5070`
  (350,155,395,212) the large right panel (description); `0x5c5080` (350,159,395,19) and
  `0x5c5090` (350,185,395,182) its heading and body. Bars 2-4 call `0x4c26d0`/`0x4c2590`/
  `0x4c2530`, which read PC fields `0x101`/`0x102` (background, background text). Exactly what
  bars 2-4 and the top-right line show is not confirmed yet.
- **Cycling**: current index at `0x5c5308`, count at `0x5c361c`, wraps; index 0 is "Create your
  own character" (`CreateYourOwn.ART`, `MainMenu.mes` 650, button text 610 "Create Character").
  The game resolves pre-mades as prototypes `16066 + index`.
- **Data**: 12 pre-made characters in `Players/G_<guid>.mpc` (~1 KB each) with `<guid>.bmp`
  (portrait) and `<guid>_b.bmp` (large picture). The `.mpc` contains the name (two files have
  placeholder names "HOrcFemale"/"HElfFemale") and an auto-level scheme string. `MainMenu.mes`
  550-562: global flag set when each is picked (550 = create your own, -1). Stories: background
  number -> `Rules/backgrnd.mes` (pre-mades at 390-500, first line = description id 1039-1050)
  -> `mes/gameback.mes` (title line + paragraphs with tab indents).
- **Object format, confirmed on all 12 `Players/*.mpc` (each parses to the exact byte):**
  - `.mpc` = `u32 object_len`, the object (`object_len` bytes), then two strings: the auto-level
    scheme and the character's **name** (the name is not an object field there).
  - Object (instance) = `u32 version` (0x77) · prototype ObjectID (24 bytes) · ObjectID (24) ·
    `u32 type` (15 = PC) · `u16 stored field count` · field bitmap (5 × u32 = 160 bits for a PC;
    popcount = the count) · values for the set bits, in bit order.
  - ObjectID (24 bytes) = `u16 kind` (-1 none, 1 prototype, 2 GUID) · `u16 pad` · `u32 pad` ·
    16-byte payload (prototype number in the first u32, or the GUID). Pre-mades point at
    prototypes 16087-16098; their GUIDs match the file names.
  - Bitmap bits: 0-35 common object fields, 64-96 critter, 128-152 PC. (Not "grouped by 64":
    each block starts at the next 32-bit boundary; see "Format knowledge", objects.)
  - Value encodings: `i32` = 4 bytes. `i64` = `u8 present` + 8 bytes if present. Sizeable array
    = `u8 present`; if present `u32 elem_size` · `u32 count` · `u32 array_id` · `count * elem_size`
    bytes · bitset (`u32 n` then `n` × u32). Strings (in the trailer) = `u32 len` incl. the NUL ·
    bytes.
  - Types seen per bit: i32: 0, 2, 3, 13, 18, 19, 26, 28, 64, 65, 70, 72, 76, 85, 89, 128.
    i64: 1 (location), 78, 83, 87, 88. Arrays: 5, 6, 16, 66, 67, 68, 69, 74, 75, 84 (24-byte
    elements: object ids), 130, 131, 134-138, 140, 147, 148 (empty in these files).
  - Bit 66 = base stats (28): 0-7 ST DX CN BE IN PE WP CH, 17 level, 25 age, 26 gender (1 male,
    0 female), 27 race (0 human, 1 dwarf, 2 elf, 3 half-elf, 4 gnome, 5 halfling, 6 half-orc,
    7 half-ogre). 67 basic skills (12), 68 tech skills (4), 69 spell/tech (25). 74/75 look like
    effects and their causes (race effect 64-71; 330 with cause 9 on some).
- **Background lives in the prototypes, not the `.mpc`:** the game install has loose prototypes
  in `<ARCANUM_DIR>/data/proto/` (1423 files: `016086-016098 - PC.pro`, NPCs, items, ...). This
  folder is **not mounted by our VFS yet**. Lugard's prototype (`016094 - PC.pro`, 954 bytes)
  contains 46 (background; `Rules/backgrnd.mes` 460) and 1046 (story id, `mes/gameback.mes`)
  next to each other, plus his name. Prototype header: version 0x77 · prototype ObjectID (none,
  kind -1) · ObjectID (kind 1, proto 16094) · `u32 type` · `u16 0x11` · bitmap; but the popcount
  of a 5-word bitmap is 18, not 17, so the prototype field encoding still needs working out.
- **Field list = the engine's own:** Arcanum.exe contains all 338 `obj_f_*` names (in reverse
  enum order in the string table; `tools/object_probe/obj_field_names_reversed.txt`). A PC object
  is three blocks: common `current_aid..pad_i64as_1` (36 fields, bitmap bits 0-35), critter
  `critter_flags..critter_pad_i64as_1` (33, bits 64-96), pc `pc_flags..pc_pad_i64as_1` (25, bits
  128-152). `pc_background` = enum 257 = 0x101, as the draw code uses.
- **Types from names** (`tools/object_probe/fields.py`): `*_idx`, `*ias_*`, `*i64as_*`,
  `overlay_fore/back`, `underlay`, `overlay_light_*`, `pc_global_flags/variables` = sizeable
  arrays; `location`, `critter_teleport_dest` = i64; `critter_fleeing_from`, `critter_gold` = object
  reference (`u8 present` + 24-byte ObjectID); `pc_player_name` = string (`u8 present` + `u32 len`
  + bytes); everything else i32. With this table **all 12 `.mpc` parse to the exact byte**
  (`python3 tools/object_probe/walk2.py` from the scratch layout; it reads `players/*.mpc`).
- **Prototypes (`data/proto/*.pro`): solved; all 13 PC prototypes and all 12 `.mpc` parse to
  the exact byte** with `tools/object_probe/walk2.py`. Rules: header = `u32 version` (0x77) ·
  prototype ObjectID (none) · ObjectID (kind 1, proto number) · `u32 type` · 20-byte bitmap at 56
  (a "fields set in this prototype" mask, bit layout as for instances; NOT a filter: walking only
  set bits fails) · then **every** field of the type in enum order from offset 76.
  Corrections to the name-based types: `critter_arrows/bullets/power_cells/fuel` are object
  references like `critter_gold`. String fields = `u8 present` · `u32 len` · `len` bytes · a NUL
  not counted in `len` (trailer strings in `.mpc` count their NUL instead).
- **Pre-made data from the prototypes** (`016086` is the empty "create your own" template):
  `description` = 16066 + index (the `0x3ec2 + index` the picker code uses); names, backgrounds
  39-50 with story ids 1039-1050 (`mes/gameback.mes`), portraits 1002-1033, race = stat 27,
  gender = stat 26. The two `.mpc` placeholder names are Clarisse Vorak (16097) and Ker'ee
  Melange (16098) in the prototypes.
- Next: mount `<ARCANUM_DIR>/data` in the VFS (proto, Players), then the Rust reader
  (`arcanum_formats::object`), then fill the picker.

## Character screen fonts

From Arcanum.exe (fonts built at 0x540a24-0x540fb0 with `0x504330(art, ...)` + `0x5352e0`; a font
is an interface art id plus one tint colour): the picker and creation screens use
- 0x64c210: art 229 `Flare12Font` (15 px), (250,250,250): story text and stat rows (`UiFont::Body`);
- 0x64c214: art 27 `morph15font`, (250,250,250): headings ("Initial Stats", "Race");
- 0x64c218: art 229, (255,50,50): bars 2-4 (gender, race, background title);
- 0x64c21c: art 27, (255,50,50): the name bar.
Colours as `menu_window::SCREEN_WHITE` / `SCREEN_RED`. The red/white channel order is inferred
(other slots in the same routine come out gold (255,210,0) and brown (0x61,0x3d,0x2a) in that
order). Other slots there, not yet used by us: art 327 `Morph30Font` in black/brown/dark red
(0x5c4040), art 171 `Cloister18Font` (grey 200 and red, 0x5c4064), art 229 in gold (0x64c240).

## Character creation (New Character)

`AppState::CreateCharacter`, `create_character.rs` (screen) + `arcanum_rules` (rules, no Bevy).
The screens hold no sheet: `character_host.rs`'s `CharacterHost` (stand-in for the host until
netcode exists) owns it; screens send `CharacterAction`s and draw `host.sheet()`. Next on the
editor or the picker stores a `CharacterBlueprint` in `host.finished`.
From Arcanum.exe window index 12 (record 0x5c4e28, background 765 `CreateCharacterBase`, buttons
0x5c4c48, click handler 0x544ff0 via jump table 0x54528c, draw 0x5447b0/0x5448e0, Next validator
0x545780):
- Buttons (screen coords): Next/Back in the bottom bar; portrait arrows 283/284 at 43,118 /
  244,118; gender 757/758 (`MPCycleLeft/RightButton`, 23x23) at 42,281 / 255,282 (both toggle);
  race at 42,331 / 255,332; background at 42,381 / 255,382.
- Race steps 0-7 with wrap, skipping races without women when female (race table 0x5c5170: only
  Human, Elf, Half-Elf, Half-Orc allow female); race or gender change resets the portrait to the
  first. Backgrounds: `Rules/backgrnd.mes` blocks of 10, legal by race-gender code, `NPC` ones are
  the pre-mades' and never offered (the exact step helpers 0x4c2300/0x4c2390/0x4c2420 were not
  decoded; we reset the background on race/gender change). Portraits: `gameport.mes` stems named
  `<code><n>` (`DWM1`..`DWM4`).
- Name: our `text_field::TextField` (hover lightens the text, click focuses: placeholder
  clears and a `|` caret blinks every 0.5 s; click elsewhere/Enter/Escape unfocuses and an empty
  field shows "Choose Name" again; Escape on a focused field does not leave the screen,
  `FocusedTextField::captures_escape`). Original: text field in bar 1 (0x5461a0), buffer of 23 bytes incl. NUL (we allow 22 characters);
  Next refuses an empty or default name with `MainMenu.mes` 506 (shown in the help box here,
  a popup in the original), then stores the name (field 0x10e). The original then opens window 14
  (record 0x5c5370, untraced; probably the character editor); we go to the loading screen.
- Stats panel: "Initial Stats" (739); rows "%s: %d" (`stat.mes` names) for 15 stats in order
  0x5c5130 (ST CN DX BE IN WP PE CH, then carry weight .. reaction modifier) at rects 0x5c4f50.
  We show the 8 primaries (8 + race effect `rules/effect.mes` 64+race + background effect) in two
  columns (346 and 466; the original's 50-pixel columns at 346/406 assume narrower text); the 7
  derived stats need formulas we do not have yet.
- Large panel: background story when the background's description > 1000, else "Race" (745) and
  770 + gender + 2 × race.
- Defaults: the original creates proto 16066 (not in `data/proto`); we start as the template
  16086 suggests: human male, first portrait, no background, no name.
- Not yet: derived stats, the hover modes that switch the large panel (0x5456b0), window 14,
  long bar texts (centred and clipped; the original left-aligns when too wide).

## Character editor (stats page after creation)

`AppState::CharacterEditor`, `character_editor.rs`; rules in `arcanum_rules`. Only after New
Character (the picker's Next starts the game directly, 0x5459f0 -> 0x5412d0).
- Original flow: creation's Next opens window 14 (record 0x5c5370, no background), whose refresh
  0x545df0 opens the in-game character screen 0x5597c0 over the menu; its Next goes to window 15
  (record 0x5c5468, untraced; then 0x5412d0 starts the game); Previous returns to creation with
  the character kept. We go from the editor straight to loading.
- Screen: `Char_Maint.art` (art 22, 800x400 at y=41); +/- `Char_Plus`/`Char_Minus` (24/23, 19x19,
  4 frames). Window coordinates (screen y = +41): info name (212,15), Level/Points/XP Next
  (212,34/53/72), Race/Gender/Age (337,...); stat values centred x 211 (ST CN DX BE) / 350
  (IN WP PE CH), y 115/155/195/235, buttons at x 170/224 and 313/367, y+2; HP (447,143) and
  fatigue (447,213) with buttons at 409/465; "Derived Stats" (51,282,253,18) rows x 49/178,
  y 308-362 (stat.mes 8-15); "Resistance" (315,282,136,18) rows x 322, y 301-365 (charedit
  12-16). Hover rects and the rest: draw 0x55b150, values 0x55b280, derived 0x55b1b0, top panel
  0x55a240, +/- handlers 0x55a80a / 0x55a997, init 0x55e110.
- Rules: 5 starting points; 1 point per step (table 0x5b52e0); min 1; race maximums (0x4b0fd0:
  20, dwarf ST/CN 21, elf DX/BE/WP 21, half-elf DX 21, gnome WP 22, halfling DX 22, half-orc
  ST/CN 21, half-ogre ST 24); a stat cannot go below its starting value (charedit 19). HP = 4 x
  bought + WP + 2 x (level + ST) + 4; fatigue the same with CN. Carry 500 x ST; damage bonus
  ST-10 (halved when negative, doubled at ST >= 20); AC DX-10; speed DX (+5 at 20); heal
  (CN+1)/3; poison recovery CN; reaction from BE (table 0x5b5330); followers CH/4 (+1 with a
  skill, not modelled). Resistances from race/background effects. XP Next 2100 at level 1.
- Not yet: the skills/tech/spells tabs (top-right buttons 527/595/663,11 and the panel at
  503,104), the portrait, alignment, hover help texts (charedit 100-140), magick-tech aptitude,
  window 15.

## Character rules (skills, tech, magick)

From Arcanum.exe (2026-11-15, Sonnet sub-agents with objdump/REA; numbers cross-checked against the
data where it has them). Code: `crates/arcanum_rules`. Engine stat ids: 4 IN, 6 WP, 16 magick/tech
aptitude, 17 level, 18 XP, 21 unspent points, 22 spells known ("magick points"), 23 tech degrees
("tech points"). Accessors: `stat_base_get` 0x4b0740 (derives 8-16), `stat_level_get` 0x4b0490
(+ effects, clamped by min 0x5b5194 / max 0x5b5204 / race 0x4b0fd0), `stat_base_set` 0x4b0980.
Editor refusal texts: global `0x64d3c4 + 4k` = charedit 17+k (init 0x55e1c9); "raise X first" by
stat via 0x55f200: ST 32, DX 33, CN 34, BE 35, IN 26, PE 37, WP 36, CH 38.

**Stats, effects, level**
| Rule | Value | Address |
|---|---|---|
| Stored base stats exclude race | All 12 pre-made prototypes store 8 in ST..CH, level 1, unspent 5, age 20; race is effect 64+race with cause 0, female gender effect 330 with cause 9 | 0x4b0980 cases 0x1a/0x1b |
| Race/gender change at creation | No refund (nothing spent there); the effect is swapped in place | 0x545490/0x5455d0, 0x441c70 |
| XP table | `rules/xp_level.mes`, line n = XP for level n (2: 2100, 3: 4600, ..., 50: 1,300,000; 51 only in Arcanum4.dat) | loader 0x4a6620, need 0x4a68b0 |
| Points per level | +1 per level, +1 more when level % 5 == 0 | 0x4a6980, 0x4a69c0 |
| Level cap | 50 in practice (level clamp 0..51, but the loop rewrites XP to level 50's threshold) | 0x4a6a6b |
| Max HP | 4·bought + field 0x1c + WP + 4 + 2·(level + ST), then effects | 0x43d5a0 |
| Aptitude | `(50·spells − 55·degrees)/10` + the sector's magick/tech aura (sector+0x4644) + effects, clamped −100..100; derived, never stored | 0x4b0740 (0x4b08ce) |

**Skills** (basic: field 0xdd[12]; tech skills: 0xde[4])
| Rule | Value | Address |
|---|---|---|
| Storage | bits 0-5 points, bits 6-7 training rank (0 none, 1 Apprentice, 2 Expert, 3 Master) | 0x4c5fa0/0x4c6000, 0x4c6680/0x4c66e0 |
| Level | points × 4 (+ effects), clamped 0..20, then capped by the governing stat; Melee is 20 when 0x45f730 (untraced) | 0x4c5eb0, 0x4c65e0 |
| Cost | 1 point per step, refund 1 | 0x4c64b0, 0x4c6af0 |
| Governing stat | Bow, Dodge, Melee, Throwing, Backstab, Pick Pocket DX; Prowling, Spot Trap PE; Gambling IN; Haggle WP; Heal IN; Persuasion CH; Repair IN, Firearms PE, Pick Locks DX, Disarm Traps PE | 0x5b6f04, 0x5b6f34 |
| Cap by stat | stat 1-5 → 3, 6-8 → 7, 9-11 → 11, 12-14 → 15, 15-17 → 19, 18-20 → 20; a point is accepted only if cap ≥ points × 4 (so 0..5 points) | 0x4c5f70, table 0x5b6fa0 |
| Training | rank r needs skill level [0,1,9,18][r]; only one step up at a time; no stat check; set by the trainer path, never by the editor | 0x4c6170, 0x4c6850, 0x5b6f44, 0x57ac90 |
| Refusals | + : 18 no points, 21 level already 20, else "raise <stat> first"; − : 23 at the editor-open value, 24 no points, 25 minimum | 0x57acd0, 0x55d640, 0x57aeb0 |
| Stat lowering | refused with 28 if a governed skill's points × 4 > cap(new stat) | 0x4c6b50 |
| Percent in play (partial) | Dodge 2L; Bow, Melee, Prowling 5L+25 (0 at L 0); Throwing 7L+25; Spot Trap 4L; Haggle 4L+10; Heal 5L | 0x4c62e0 |

**Tech** (field 0xdf `critter_spell_tech_idx` [17..24] = disciplines 0-7, degree 0-7)
| Rule | Value | Address |
|---|---|---|
| Cost | 1 point per degree (table [0,1,1,1,1,1,1,1]); +1 stat 23 | 0x5b5124, 0x589a20, 0x4afec0 |
| IN per degree | [0,5,8,11,13,15,17,19]; no level requirement | 0x5b5164 |
| Learned schematics | degree d gives `rules/schematic.mes` 2000 + 200·discipline + 10·(d−1) | UI 0x56d1b2 |
| Found schematics | using the schematic item adds its id to `pc_schematics_found_idx` (0x10b); no requirement | 0x56bb60 |
| Crafting | needs complexity [0,10,20,35,50,65,80,100][effective degree] ≥ −(result field 0x60); effective degree drops while IN is below its requirement | 0x4b00b0 |
| Refusals | 26 on + when IN too low (or at degree 7); 31 when lowering IN below a degree's requirement (order 28, 29, 31) | 0x4b02b0 |

**Magick** (field 0xdf [0..15] = known count per college, a prefix; [16] = mastery college, −1 none)
| Rule | Value | Address |
|---|---|---|
| Order | buy only the next spell of a college; sell only the top one | 0x4b1790, 0x4b19b0 |
| Cost | 1 point per spell; +1 stat 22 | 0x4b1650, 0x57c540 |
| Requirements | level ≥ [1,1,5,10,15][rank]; IN ≥ 5; WP ≥ [6,9,12,15,18][rank] (SpellList `IQ:` would override; the data has none) | 0x5b5770, 0x4502e0, 0x5b0de0 |
| Refusals | 18 → 30 (level) → 26 (IN) → 36 (WP, also the fallback); 27 selling an accepted spell; 29/39 lowering IN/WP below a known spell's need | 0x55f160.., 0x4b1b90, 0x4b1c00 |
| Mastery | [16] = mastered college (−1 none, set at 0x4b1530); settable once, for a college at 5 spells; earned in play through quest lines, never in the editor (owner, 2026-11-15) | 0x4b1cb0 |

**Starting kit**
- Created characters: background money (line 4, mostly 400) and items (line 5, 9 backgrounds).
  The code that hands them out was not found; our blueprint uses them.
- Pre-mades: `critter_inventory_source` → `Rules/InvenSource.mes` (`Name: min,max chance,proto ...`).
  Lugard = 24 "General(sm)PC": 385 gold, 6029, 8168, 8065. All PC sources are fixed gold with only
  100% items.
- Auto-equip: not confirmed (module `Rules/AutoEquip.mes` exists; no call path from window 15).

**Copied oddities** (behaviour that looks like a bug; copy, owner decides later)
- Spell `IQ:`/MinIQ values are checked against Willpower; Intelligence is a flat 5.
- A spell rank-order failure shows "raise Willpower first" (36), the fallback message.
- Spell buy at a full college computes the next college's first spell; no bounds guard found.
- Tech degrees can be sold below the accepted level (the snapshot is written, never read).
- Tech + at degree 7 shows "raise Intelligence first" (reads past the cost table).
- Lowering-IN check for tech uses the raw new IN in the UI, the effective IN in the setter.
- Skill − at the accepted value shows 23 and may still lower it (fall-through, unconfirmed).
- Basic skill − refunds until the level changes (several points); tech skill − one step.
- Training rank is never lowered when skill points drop (to fix later: `TODO.md` → Rules).
- Level 51 unreachable; at level 50 XP is clamped to 1,300,000 on every check.

**Open**: who hands out background money/items; fatigue maximum formula; training rank effects
(0x4c62e0, flags 0x5b6f64); the sector aura source; which quest/dialog path calls 0x4b1cb0 to grant
mastery. (Index 16 settled: the mastery college; tech code skips it, which is why the tech report
called it unused.)

## Known issues / backlog (not started)

- Sector objects: instance fields fall back to the prototype when the instance bitmap omits them
  (3,799 of 121,185 start-map scenery objects have no `current_aid`). `Object::parse_prototype`
  only supports pc. Wall, portal and scenery prototype parsing is needed before the art step
  (so every object resolves art) and the blocking step (prototype flags).
- Blocking: `Sector::blocked` is only the tile blocking mask (`None` for version < 4: 32 of 621
  start-map sectors; sparse: 172 of 621 set any bit, the start sector none). Objects (walls,
  scenery, closed portals) block as well and are not decoded for blocking yet.

- Sprites use linear filtering, so scaled 800x600 art looks soft. Consider nearest or a scaler.
- Windows narrower than 4:3 crop the menu art (they are rejected by resolution validation anyway).
- Label positions in `main_menu.rs` are hand-tuned constants for the original art.
- `BitmapText` wraps (`max_width`), breaks at real or escaped `\n`, indents leading tabs (4
  spaces per stop) and aligns left or centre (`BitmapFont::layout`/`render_block`,
  `ArcanumFonts::insert_paragraph`). No vertical clipping or scrolling yet: a long story overflows
  the New Game description panel. Characters outside Windows-1252 render as `?`.
- Bitmap text is upscaled with linear filtering (×1.8 at 1080p), so it is slightly soft.
- Options sliders only step with the arrows (no click or drag on the tube). The bottom help box
  and the round lens (`OptionsPCLens.art`) on the options art are unused.
- `.art` loaders register `art`, `ART` and `Art`: Bevy's extension match is case-sensitive.
- Hover and click are tested headlessly by inserting `Hovered` and triggering `Activate`. The
  real pointer path is only checked manually: a screenshot run with the mouse over the window
  showed the entry under the pointer highlighted.
- No keyboard navigation of the menu, and no hotkeys from `MainMenu.mes`.
- Config loading is lenient: a bad or unknown key is skipped with a warning (logged at startup),
  out-of-range sliders are clamped; the file is rewritten on leaving Options.
- Steam install folder is not auto-detected; players must set `ARCANUM_DIR`.

## Suggested next steps

1. Check the stats page by eye (see Handoff), then its remaining parts: the skills / tech /
   spells tabs (top-right buttons and the right-hand panel), portrait, alignment gauge, hover
   help (charedit 100-140); then the original's window 15 between the editor and the game.
2. The seven derived stats on character creation's "Initial Stats" panel (formulas are now in
   `arcanum_rules::RulesData::derived`), the picker's stat values (it shows "Initial Stats" with no values),
   and story scrolling for long texts.
3. Use the chosen/created character when the game starts (`ChosenCharacter`,
   `CreatedCharacter`), once there is a world.
4. Make the remaining settings take effect once their systems exist (combat, floating text).
   Dialogue must play voice through `audio::play_voice`. Menu keyboard hotkeys
   (`MainMenu.mes` 20-24, 60-64).
5. Credits screen (multi-line text now exists).
6. `.art` animation playback (frame rate, rotations, center/delta offsets) as a Bevy component.
7. Replacement-art layer: a PNG next to an asset path (e.g. `mainmenuback.png`) overrides the
   `.art`, so new art and upscales can drop in. Plus a coverage report of replaced assets.
8. Steam install auto-detection (libraryfolders.vdf) as the default `ARCANUM_DIR`.
9. Later: maps/tiles, critters, dialogue, scripts, saves.

## Art pipeline (outside the repo)

The owner is producing new and upscaled art in ComfyUI (`~/ComfyUI`, M3 Max, 48 GB).
`4x-UltraSharp` is linked into `~/ComfyUI/models/upscale_models`. Planned workflow: SDXL with the
original menu background as a low-strength depth ControlNet reference, then Ultimate SD Upscale
and a 4x-UltraSharp pass to 3840x2160. The extracted original background is at
`assets/extracted/mainmenuback.png` (outside git).

## Working with the owner

- Short, factual responses. No filler.
- Commit and open PRs only when asked. New work goes on a new branch; PRs have been stacked.
- Commit messages end with the `Co-Authored-By` line the harness specifies; PR bodies end with
  the Claude Code footer.
- PR review comments are left on GitHub; check all open PRs when asked to address them.
