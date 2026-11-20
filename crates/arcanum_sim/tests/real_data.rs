//! Runs against the real game data; skipped when `assets/` is absent.

use std::path::PathBuf;

use arcanum_formats::map::TileLoc;
use arcanum_sim::StartMap;
use arcanum_vfs::Vfs;

/// PATCH0, then Arcanum.dat, then the game folder's archives (as in `arcanum_formats`' tests).
fn vfs() -> Option<Vfs> {
    let game = std::env::var_os("ARCANUM_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/Arcanum"));
    let modules = game.join("modules");
    if !modules.join("Arcanum.dat").is_file() {
        eprintln!("skipping: game data not found");
        return None;
    }
    let mut vfs = Vfs::new();
    let patch = modules.join("Arcanum.PATCH0");
    if patch.is_file() {
        vfs.mount_archive(patch).unwrap();
    }
    vfs.mount_archive(modules.join("Arcanum.dat")).unwrap();
    vfs.mount_archives_in(&game).unwrap();
    // The loose prototypes and pre-made characters, as the game mounts them.
    vfs.mount_dir(game.join("data"));
    Some(vfs)
}

#[test]
fn new_games_start_where_the_original_does() {
    let Some(vfs) = vfs() else { return };
    let start = StartMap::load(&vfs).unwrap();
    // The clock from rules/gameinit.mes: 1885, 15:00 ("Currently 3pm, WAS 10am").
    let clock = arcanum_sim::GameStart { year: 1885, hour: 15 };
    assert_eq!(start, StartMap { map: "Arcanum1-024-fixed".into(), position: TileLoc { x: 92958, y: 82592 }, clock });
}

#[test]
fn a_new_game_boots_on_the_start_map() {
    use std::sync::{Arc, RwLock};

    use arcanum_rules::{CharacterBlueprint, CharacterSheet, RulesData};
    use arcanum_sim::{LoadedMap, PlayerId, Position, SimApp, SimVfs, WorldState};

    let Some(vfs) = vfs() else { return };
    let start = StartMap::load(&vfs).unwrap();
    let mut world = WorldState::default();
    let sheet = CharacterSheet { name: "Virgil".into(), ..Default::default() };
    world.add_player(Some(&start), PlayerId(1), CharacterBlueprint { sheet, gold: 0, items: vec![], prototype: None });

    let mut sim = SimApp::new(RulesData::default());
    sim.use_manual_clock();
    sim.app.insert_resource(SimVfs(Arc::new(RwLock::new(vfs))));
    sim.boot(world.0.unwrap());
    sim.update();
    sim.update();

    let map = sim.app.world().resource::<LoadedMap>();
    assert!(map.errors.is_empty(), "{:?}", map.errors);
    assert!(map.properties.is_some());
    assert!(map.sectors.contains_key(&start.position.sector()), "the PC's sector");
    assert_eq!(map.sectors.len() + map.absent.len(), 9);
    eprintln!("loaded {} sectors around the start, {} absent", map.sectors.len(), map.absent.len());
    let mobiles: usize = map.mobiles.values().map(Vec::len).sum();
    let world = sim.app.world_mut();
    let pcs: Vec<TileLoc> = world.query_filtered::<&Position, bevy_ecs::query::With<arcanum_sim::PlayerControlled>>().iter(world).map(|p| p.0).collect();
    assert_eq!(pcs, vec![start.position]);
    let npcs: Vec<(TileLoc, u32)> = world.query::<(&Position, &arcanum_sim::Npc)>().iter(world).map(|(p, n)| (p.0, n.prototype)).collect();
    eprintln!("{mobiles} mobile objects on the map; {} NPCs round the start: {npcs:?}", npcs.len());
    let wanderers: Vec<TileLoc> = world.query::<(&Position, &arcanum_sim::Standpoints)>().iter(world).filter(|(_, s)| s.wanders).map(|(p, _)| p.0).collect();
    eprintln!("{} of them wander: {wanderers:?}", wanderers.len());
    // Virgil stands next to the PC, and the dying gnome two tiles down (start-map .mob files).
    assert!(npcs.iter().any(|&(p, _)| p == TileLoc { x: 92959, y: 82592 }), "{npcs:?}");
    assert!(npcs.iter().any(|&(p, _)| p == TileLoc { x: 92959, y: 82594 }), "{npcs:?}");
}

#[test]
fn paths_on_the_start_map_pass_the_hosts_check() {
    use std::sync::{Arc, RwLock};

    use arcanum_rules::{CharacterBlueprint, CharacterSheet, RulesData};
    use arcanum_sim::{LoadedMap, PlayerId, SimApp, SimVfs, Walkable, WorldState, check_path, find_path};

    let Some(vfs) = vfs() else { return };
    let start = StartMap::load(&vfs).unwrap();
    let mut world = WorldState::default();
    let sheet = CharacterSheet { name: "Virgil".into(), ..Default::default() };
    world.add_player(Some(&start), PlayerId(1), CharacterBlueprint { sheet, gold: 0, items: vec![], prototype: None });
    let mut sim = SimApp::new(RulesData::default());
    sim.use_manual_clock();
    sim.app.insert_resource(SimVfs(Arc::new(RwLock::new(vfs))));
    sim.boot(world.0.unwrap());
    sim.update();
    sim.update();
    let map = sim.app.world().resource::<LoadedMap>();

    assert!(map.walkable(start.position), "the start tile");
    // Every walkable tile within 20 of the start that A* reaches gets a path the host accepts.
    let (mut found, mut blocked) = (0, 0);
    for dy in -20i32..=20 {
        for dx in -20i32..=20 {
            let goal = TileLoc { x: start.position.x.wrapping_add_signed(dx), y: start.position.y.wrapping_add_signed(dy) };
            if goal == start.position {
                continue;
            }
            if !map.walkable(goal) {
                blocked += 1;
                continue;
            }
            if let Ok(path) = find_path(map, start.position, goal) {
                found += 1;
                assert_eq!(check_path(map, start.position, &path).as_ref(), Ok(&path), "to {goal:?}");
            }
        }
    }
    let edges: usize = map.blocking.edges.values().map(|e| e.count_ones() as usize).sum();
    eprintln!(
        "{found} tiles reached, {blocked} blocked, of 41 x 41 around the start; loaded: {} prototypes, {} tiles and {edges} edges blocked by objects, errors {:?}",
        map.prototypes.len(),
        map.blocking.tiles.len(),
        map.errors
    );
    assert!(map.errors.is_empty(), "{:?}", map.errors);
    assert!(map.prototypes.len() > 1000);
    assert!(found > 100, "{found}");
    assert!(blocked > 0, "the wreck and the scenery round the start block");
}

/// In a Shrouded Hills house, walls block by edge: no path crosses one, and the way in and out
/// is through the doors.
#[test]
fn house_walls_block_and_doors_let_through() {
    use arcanum_formats::map::{MAP_LIST, parse_map_list, start_map};
    use arcanum_formats::mes::MesFile;
    use arcanum_sim::{LoadedMap, Walkable, find_path, step_allowed};

    let Some(vfs) = vfs() else { return };
    let list = parse_map_list(&MesFile::parse(&vfs.read(MAP_LIST).unwrap()).unwrap()).unwrap();
    let name = start_map(&list).unwrap().name.clone();
    let mut map = LoadedMap::default();
    map.open(Some(&vfs), &name);
    let inside = TileLoc { x: 90330, y: 84122 };
    map.load_around(Some(&vfs), inside);
    let edges: usize = map.blocking.edges.values().map(|e| e.count_ones() as usize).sum();
    let doors: usize = map.blocking.diagonal_edges.values().map(|e| e.count_ones() as usize).sum();
    eprintln!("{edges} wall edges, {doors} door-opening edges, {} blocked tiles", map.blocking.tiles.len());
    assert!(edges > 100, "{edges}");
    // Every wall edge stops a step across it, both ways.
    let mut checked = 0;
    for (&at, &mask) in &map.blocking.edges {
        for dir in (1..8).step_by(2) {
            if mask >> dir & 1 == 0 {
                continue;
            }
            let (dx, dy) = arcanum_sim::DIRECTIONS[dir];
            let next = TileLoc { x: at.x.wrapping_add_signed(dx), y: at.y.wrapping_add_signed(dy) };
            assert!(!step_allowed(&map, at, next) && !step_allowed(&map, next, at), "{at:?} dir {dir}");
            checked += 1;
        }
    }
    assert_eq!(checked, edges);
    // From inside the house to well outside: there is a way (through a door).
    // The tile itself has furniture on it: start from a free one next to it.
    let inside = (0..9u32).map(|i| TileLoc { x: inside.x + i % 3 - 1, y: inside.y + i / 3 - 1 }).find(|&t| map.walkable(t)).expect("a free tile");
    let outside = TileLoc { x: inside.x + 12, y: inside.y + 12 };
    let path = find_path(&map, inside, outside);
    eprintln!("inside to outside: {:?} steps", path.as_ref().map(Vec::len));
    assert!(path.is_ok(), "{path:?}");
}
