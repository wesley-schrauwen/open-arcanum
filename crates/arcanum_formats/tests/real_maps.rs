//! Runs against the real game data; skipped when `assets/` is absent.
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use arcanum_formats::art_id::{ArtId, ArtIndex};
use arcanum_formats::map::{MAP_LIST, MapProperties, SectorId, TileLoc, parse_map_list, start_map};
use arcanum_formats::mes::MesFile;
use arcanum_formats::object::{Object, Value};
use arcanum_formats::sector::Sector;
use arcanum_vfs::{ArcanumArchive, Vfs};

/// `ARCANUM_DIR`, or `assets/Arcanum` at the workspace root.
fn game_dir() -> PathBuf {
    std::env::var_os("ARCANUM_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/Arcanum"))
}

/// PATCH0, then Arcanum.dat, then the game folder's archives (highest priority first).
fn vfs() -> Option<Vfs> {
    let game = game_dir();
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
    Some(vfs)
}

#[test]
fn start_map_from_map_list() {
    let Some(vfs) = vfs() else { return };
    let mes = MesFile::parse(&vfs.read(MAP_LIST).unwrap()).unwrap();
    let list = parse_map_list(&mes).unwrap();
    let start = start_map(&list).unwrap();
    assert_eq!(start.name, "Arcanum1-024-fixed");
    assert_eq!(start.start, TileLoc { x: 92958, y: 82592 });
}

#[test]
fn start_map_properties() {
    let Some(vfs) = vfs() else { return };
    let mes = MesFile::parse(&vfs.read(MAP_LIST).unwrap()).unwrap();
    let list = parse_map_list(&mes).unwrap();
    let start = start_map(&list).unwrap();
    let prp = MapProperties::parse(&vfs.read(&format!("maps/{}/map.prp", start.name)).unwrap()).unwrap();
    assert_eq!(prp.width, 128_000);
    assert!(prp.contains(start.start));
}

#[test]
fn every_map_list_entry_has_a_folder() {
    let Some(vfs) = vfs() else { return };
    let mes = MesFile::parse(&vfs.read(MAP_LIST).unwrap()).unwrap();
    let list = parse_map_list(&mes).unwrap();
    // Entries observed to have no folder in the shipped data.
    let allowed: &[&str] = &["Network Test", "Multi2"];
    let mut missing = Vec::new();
    for e in &list {
        let path = format!("maps/{}/map.prp", e.name);
        if vfs.exists(&path) {
            MapProperties::parse(&vfs.read(&path).unwrap()).unwrap_or_else(|err| panic!("{path}: {err}"));
        } else {
            missing.push(e.name.as_str());
        }
    }
    eprintln!("map list entries without a folder: {missing:?}");
    for m in &missing {
        assert!(allowed.contains(m), "{m} has no maps/<name>/map.prp");
    }
}

const TYPE_NAMES: [&str; 18] = [
    "wall", "portal", "container", "scenery", "projectile", "weapon", "ammo", "armor", "gold", "food", "scroll", "key",
    "key_ring", "written", "generic", "pc", "npc", "trap",
];

/// Object counts by type name.
fn by_name(counts: &BTreeMap<u32, usize>) -> BTreeMap<&'static str, usize> {
    counts.iter().map(|(t, n)| (TYPE_NAMES[*t as usize], *n)).collect()
}

/// `modules/Arcanum.dat` alone (the counts below are for the unpatched archive).
fn module_archive() -> Option<ArcanumArchive> {
    let path = game_dir().join("modules/Arcanum.dat");
    if !path.is_file() {
        eprintln!("skipping: game data not found");
        return None;
    }
    Some(ArcanumArchive::open(path).unwrap())
}

/// Entries of `archive` under `maps/` ending in `ext` (lowercase).
fn map_files(archive: &ArcanumArchive, ext: &str) -> Vec<String> {
    archive
        .entries()
        .iter()
        .filter(|e| !e.is_dir())
        .map(|e| e.path.clone())
        .filter(|p| {
            let p = p.to_ascii_lowercase().replace('\\', "/");
            p.starts_with("maps/") && p.ends_with(ext)
        })
        .collect()
}

/// Every mobile object file (`maps/*/G_*.mob`, one instance object each) parses to the exact
/// byte. Sectors hold only walls, portals, scenery and traps, so this is the real-data check of
/// the container, item and NPC tables.
#[test]
fn every_mob_object_parses() {
    let Some(archive) = module_archive() else { return };
    let files = map_files(&archive, ".mob");
    let mut counts = BTreeMap::new();
    let mut failures = Vec::new();
    for path in &files {
        let name = path.rsplit(['\\', '/']).next().unwrap();
        assert!(name.starts_with("G_"), "{path}");
        match Object::parse_instance(&archive.read(path).unwrap()) {
            Ok(obj) => {
                // The object's GUID is the file name.
                assert!(path.to_ascii_uppercase().contains(&obj.id.guid().unwrap()), "{path}");
                *counts.entry(obj.object_type).or_insert(0) += 1;
            }
            Err(e) => failures.push(format!("{path}: {e}")),
        }
    }
    eprintln!("{} .mob objects: {:?}", files.len(), by_name(&counts));
    assert!(failures.is_empty(), "{} failed, first: {:?}", failures.len(), &failures[..failures.len().min(5)]);
    let expected = [
        ("ammo", 422),
        ("armor", 1437),
        ("container", 915),
        ("food", 2132),
        ("generic", 1095),
        ("gold", 797),
        ("key", 97),
        ("key_ring", 2),
        ("npc", 3792),
        ("scroll", 180),
        ("weapon", 1718),
        ("written", 136),
    ];
    assert_eq!(by_name(&counts), BTreeMap::from(expected));
    assert_eq!(files.len(), 12_723);
}

/// Adds `sector`'s objects to `counts` by type.
fn count_types(sector: &Sector, counts: &mut BTreeMap<u32, usize>) {
    for o in &sector.objects {
        *counts.entry(o.object_type).or_insert(0) += 1;
    }
}

#[test]
fn start_sector_parses_to_the_byte() {
    let Some(vfs) = vfs() else { return };
    let data = vfs.read("maps/Arcanum1-024-fixed/86570436012.sec").unwrap();
    assert_eq!(data.len(), 47291);
    let sector = Sector::parse(&data).unwrap();
    assert!(!sector.objects.is_empty());
    eprintln!("start sector: version {}, {} lights, {} objects", sector.version, sector.lights.len(), sector.objects.len());
}

/// Every object's `location` in the start sector decodes to a tile inside that sector.
#[test]
fn start_sector_object_locations_are_inside_it() {
    let Some(vfs) = vfs() else { return };
    let id = SectorId(86_570_436_012);
    let sector = Sector::parse(&vfs.read(&format!("maps/Arcanum1-024-fixed/{}", id.file_name())).unwrap()).unwrap();
    let mut located = 0;
    for o in &sector.objects {
        let Some(Value::Int64(Some(packed))) = o.field("location") else { continue };
        assert_eq!(TileLoc::from_packed(*packed).sector(), id, "object type {}", o.object_type);
        located += 1;
    }
    assert!(located > 0);
    eprintln!("start sector: {located} of {} objects have a location", sector.objects.len());
}

/// Every object in all 621 start-map sectors parses (through the VFS, so with the patch).
#[test]
fn start_map_sector_objects_parse() {
    let Some(vfs) = vfs() else { return };
    let files: Vec<String> =
        vfs.list_dir("maps/Arcanum1-024-fixed").unwrap().into_iter().filter(|p| p.to_ascii_lowercase().ends_with(".sec")).collect();
    assert_eq!(files.len(), 621);
    let mut counts = BTreeMap::new();
    for path in &files {
        let sector = Sector::parse(&vfs.read(path).unwrap()).unwrap_or_else(|e| panic!("{path}: {e}"));
        count_types(&sector, &mut counts);
    }
    eprintln!("start map sector objects: {:?}", by_name(&counts));
    assert!(counts.keys().all(|t| [0, 1, 3, 17].contains(t)), "only walls, portals, scenery and traps: {counts:?}");
}

/// Every `.sec` of every map in `Arcanum.dat` parses to the exact byte, and every one in
/// `Arcanum.PATCH0` if present.
#[test]
fn every_sector_of_every_map_parses() {
    let Some(archive) = module_archive() else { return };
    let check = |archive: &ArcanumArchive| {
        let files = map_files(archive, ".sec");
        let mut counts = BTreeMap::new();
        let mut failures = Vec::new();
        for path in &files {
            match Sector::parse(&archive.read(path).unwrap()) {
                Ok(sector) => count_types(&sector, &mut counts),
                Err(e) => failures.push(format!("{path}: {e}")),
            }
        }
        assert!(failures.is_empty(), "{} failed, first: {:?}", failures.len(), &failures[..failures.len().min(5)]);
        (files.len(), counts)
    };
    let (files, counts) = check(&archive);
    eprintln!("Arcanum.dat: {files} sectors, objects {:?}", by_name(&counts));
    // 1680 files: the research walk saw 1668 (its extraction missed `Castle S'nel N'fa` and
    // `Roseborough Inn2`, 6 sectors each).
    assert_eq!(files, 1680);
    assert_eq!(by_name(&counts), BTreeMap::from([("portal", 2107), ("scenery", 170_719), ("trap", 2063), ("wall", 89_013)]));
    let patch = game_dir().join("modules/Arcanum.PATCH0");
    if patch.is_file() {
        let (files, counts) = check(&ArcanumArchive::open(patch).unwrap());
        eprintln!("Arcanum.PATCH0: {files} sectors, objects {:?}", by_name(&counts));
    }
}

fn art_index(vfs: &Vfs) -> ArtIndex {
    ArtIndex::load(|p| vfs.read(p).ok()).unwrap_or_else(|e| panic!("art index: {e}"))
}

/// Resolution outcome counts per art kind name.
#[derive(Default)]
struct ArtStats {
    resolved: usize,
    /// `0xFFFFFFFF`.
    no_art: usize,
    /// The index has no path for the id.
    unresolved: BTreeMap<u32, usize>,
    /// The path does not exist in the VFS.
    missing: BTreeMap<String, usize>,
}

fn check_art(vfs: &Vfs, index: &ArtIndex, cache: &mut HashMap<u32, Option<String>>, stats: &mut BTreeMap<String, ArtStats>, id: ArtId) {
    let s = stats.entry(format!("{:?}", id.kind())).or_default();
    if id.is_none() {
        s.no_art += 1;
        return;
    }
    let path = cache.entry(id.0).or_insert_with(|| index.path(id));
    match path {
        None => *s.unresolved.entry(id.0).or_insert(0) += 1,
        Some(p) if vfs.exists(p) => s.resolved += 1,
        Some(p) => *s.missing.entry(p.clone()).or_insert(0) += 1,
    }
}

/// Checks every tile, roof, light and object art id of `sectors`; returns the stats per kind and,
/// per object type, how many objects have no instance `current_aid` (the engine falls back to
/// the prototype's value for those, which is not decoded yet, so their art is not checked).
fn art_stats(vfs: &Vfs, sectors: &[String]) -> (BTreeMap<String, ArtStats>, BTreeMap<u32, usize>) {
    let index = art_index(vfs);
    let (mut cache, mut stats) = (HashMap::new(), BTreeMap::new());
    let mut no_aid = BTreeMap::new();
    for path in sectors {
        let sector = Sector::parse(&vfs.read(path).unwrap()).unwrap_or_else(|e| panic!("{path}: {e}"));
        for t in sector.tiles.iter() {
            check_art(vfs, &index, &mut cache, &mut stats, t.art_id());
        }
        // Roof ids of 0xFFFFFFFF are empty cells, not missing art.
        for r in sector.roofs.iter().flat_map(|r| r.iter()).filter(|r| **r != ArtId::NONE) {
            check_art(vfs, &index, &mut cache, &mut stats, *r);
        }
        for l in &sector.lights {
            check_art(vfs, &index, &mut cache, &mut stats, l.art_id);
        }
        // Traps store no art id.
        for o in sector.objects.iter().filter(|o| o.object_type != 17) {
            match o.art_id() {
                Some(id) => check_art(vfs, &index, &mut cache, &mut stats, id),
                None => *no_aid.entry(o.object_type).or_insert(0) += 1,
            }
        }
    }
    (stats, no_aid)
}

fn report(label: &str, stats: &BTreeMap<String, ArtStats>) -> usize {
    let mut bad = 0;
    for (kind, s) in stats {
        let (u, m): (usize, usize) = (s.unresolved.values().sum(), s.missing.values().sum());
        eprintln!("{label} {kind}: resolved {}, no art {}, unresolved {u}, missing file {m}", s.resolved, s.no_art);
        bad += u + m;
        for (id, n) in s.unresolved.iter().take(5) {
            eprintln!("   unresolved {id:#010x} x{n}");
        }
        for (p, n) in s.missing.iter().take(5) {
            eprintln!("   missing {p} x{n}");
        }
    }
    bad
}

/// The 3x3 sectors around the start sector: every art id resolves to a file that exists.
#[test]
fn start_area_art_resolves() {
    let Some(vfs) = vfs() else { return };
    let centre = SectorId(86_570_436_012);
    let mut sectors = Vec::new();
    for dy in -1i64..=1 {
        for dx in -1i64..=1 {
            let id = SectorId::new((centre.x() as i64 + dx) as u32, (centre.y() as i64 + dy) as u32);
            sectors.push(format!("maps/Arcanum1-024-fixed/{}", id.file_name()));
        }
    }
    let (stats, _) = art_stats(&vfs, &sectors);
    assert_eq!(report("start area", &stats), 0);
    assert!(stats["Tile"].resolved > 0 && stats["Facade"].resolved > 0 && stats["Scenery"].resolved > 0);
}

/// All 621 sectors. Observed: 0 unresolved and 0 missing among the ids that are stored; one
/// scenery object has `current_aid = 0xFFFFFFFF`. Objects with no instance `current_aid` at all
/// are counted separately: their art comes from the prototype, which is not decoded yet.
#[test]
fn start_map_art_coverage() {
    let Some(vfs) = vfs() else { return };
    let sectors: Vec<String> =
        vfs.list_dir("maps/Arcanum1-024-fixed").unwrap().into_iter().filter(|p| p.to_ascii_lowercase().ends_with(".sec")).collect();
    assert_eq!(sectors.len(), 621);
    let (stats, no_aid) = art_stats(&vfs, &sectors);
    eprintln!("objects without an instance current_aid, by type: {no_aid:?}");
    // Scenery (type 3) only; walls (0) and portals (1) always store one.
    assert_eq!(no_aid, BTreeMap::from([(3, 3799)]));
    assert_eq!(report("start map", &stats), 0);
    assert_eq!(stats["Tile"].resolved + stats["Facade"].resolved, 621 * 4096);
    assert_eq!(stats.values().map(|s| s.no_art).sum::<usize>(), 1);
}

/// The tile blocking mask exists from version 4 on and is sparse: it is not the whole story for
/// blocking. Observed over the 621 start-map sectors: 32 have no mask, 172 set any bit.
#[test]
fn start_map_tile_mask_presence() {
    let Some(vfs) = vfs() else { return };
    let files: Vec<String> =
        vfs.list_dir("maps/Arcanum1-024-fixed").unwrap().into_iter().filter(|p| p.to_ascii_lowercase().ends_with(".sec")).collect();
    let (mut none, mut any_set) = (0, 0);
    for path in &files {
        let sector = Sector::parse(&vfs.read(path).unwrap()).unwrap_or_else(|e| panic!("{path}: {e}"));
        assert_eq!(sector.blocked.is_none(), sector.version < 4, "{path}");
        match &sector.blocked {
            None => none += 1,
            Some(m) => any_set += usize::from(m.iter().any(|w| *w != 0)),
        }
    }
    eprintln!("no mask {none}, mask with any bit set {any_set}");
    assert_eq!((none, any_set), (32, 172));
}

/// Every critter art id (type 2) in the `.mob` NPCs and in the PC prototypes names a file, and
/// the walk animation exists for every body and gender those ids use (elf women only through
/// the engine's remap to the human woman's body: no archive has `art/critter/eff/`).
#[test]
fn critter_art_resolves() {
    use arcanum_formats::art_id::{ANIM_WALK, ArtKind, CritterArt};
    let (Some(archive), Some(vfs)) = (module_archive(), vfs()) else { return };
    let index = art_index(&vfs);
    let mut ids = BTreeMap::new();
    for path in map_files(&archive, ".mob") {
        let obj = Object::parse_instance(&archive.read(&path).unwrap()).unwrap();
        if let Some(id) = obj.art_id().filter(|id| id.kind() == ArtKind::Critter) {
            *ids.entry(id.0).or_insert(0usize) += 1;
        }
    }
    let protos = game_dir().join("data/proto");
    for n in 16086..=16098 {
        let path = protos.join(format!("0{n} - PC.pro"));
        let obj = Object::parse_prototype(&std::fs::read(&path).unwrap()).unwrap();
        *ids.entry(obj.art_id().unwrap().0).or_insert(0) += 1;
    }
    let (mut missing, mut found) = (BTreeMap::new(), 0);
    for (&id, &n) in &ids {
        let art = CritterArt::from_id(ArtId(id));
        for path in [index.path(ArtId(id)).unwrap(), CritterArt { anim: ANIM_WALK, frame: 0, ..art }.path()] {
            if vfs.exists(&path) {
                found += n;
            } else {
                *missing.entry(path).or_insert(0) += n;
            }
        }
    }
    eprintln!("critter ids: {} distinct, {found} paths found, missing: {missing:?}", ids.len());
    assert!(found > 0);
    assert!(missing.is_empty(), "{missing:?}");
}

/// Every object on the start map has its prototype as a loose `.pro` (none are in the archives).
#[test]
fn start_map_prototypes_exist() {
    let Some(vfs) = vfs() else { return };
    let protos: BTreeMap<u32, String> = std::fs::read_dir(game_dir().join("data/proto"))
        .map(|rd| rd.flatten().filter_map(|e| { let n = e.file_name().to_string_lossy().into_owned(); Some((n.get(..6)?.parse().ok()?, n)) }).collect())
        .unwrap_or_default();
    let archived: Vec<String> = vfs.list_dir("proto").unwrap_or_default();
    eprintln!("{} loose prototypes, {} in archives (e.g. {:?})", protos.len(), archived.len(), &archived[..archived.len().min(3)]);
    let mut used: BTreeMap<(u32, u32), usize> = BTreeMap::new();
    for path in vfs.list_dir("maps/Arcanum1-024-fixed").unwrap().into_iter().filter(|p| p.to_ascii_lowercase().ends_with(".sec")) {
        let sector = Sector::parse(&vfs.read(&path).unwrap()).unwrap();
        for o in &sector.objects {
            if let Some(p) = o.prototype.prototype() {
                *used.entry((o.object_type, p)).or_default() += 1;
            }
        }
    }
    let mut missing: BTreeMap<u32, (usize, usize)> = BTreeMap::new();
    for (&(ty, p), &n) in &used {
        let e = missing.entry(ty).or_default();
        e.0 += n;
        if !protos.contains_key(&p) {
            e.1 += n;
        }
    }
    eprintln!("per type: (objects, objects whose prototype has no .pro): {missing:?}");
    assert!(missing.values().all(|&(_, absent)| absent == 0), "{missing:?}");
    let ranges: BTreeMap<u32, (u32, u32)> = used.keys().fold(BTreeMap::new(), |mut m, &(t, p)| {
        let e = m.entry(t).or_insert((p, p));
        e.0 = e.0.min(p);
        e.1 = e.1.max(p);
        m
    });
    eprintln!("prototype number ranges per type: {ranges:?}");
}

/// The wall pieces the original's walking treats as door openings (10, 13, 14, 17-19, 22, 25,
/// 26, 29-32) are the only ones that share their tile and edge with a portal.
#[test]
fn only_door_opening_wall_pieces_hold_portals() {
    let Some(vfs) = vfs() else { return };
    let mut portals: std::collections::HashSet<(i64, u32)> = Default::default();
    let mut walls: Vec<(i64, u32, u32)> = vec![];
    for path in vfs.list_dir("maps/Arcanum1-024-fixed").unwrap().into_iter().filter(|p| p.to_ascii_lowercase().ends_with(".sec")) {
        let sector = Sector::parse(&vfs.read(&path).unwrap()).unwrap();
        for o in &sector.objects {
            let (Some(Value::Int64(Some(loc))), Some(art)) = (o.field("location"), o.art_id()) else { continue };
            let rot = (art.0 >> 11) & 7;
            let rot = if rot % 2 == 0 { rot + 1 } else { rot };
            match o.object_type {
                0 => walls.push((*loc, (art.0 >> 14) & 0x3f, rot)),
                1 => { portals.insert((*loc, rot)); }
                _ => {}
            }
        }
    }
    let mut by_piece: BTreeMap<u32, (usize, usize)> = BTreeMap::new();
    for (loc, piece, rot) in &walls {
        let e = by_piece.entry(*piece).or_default();
        e.0 += 1;
        if portals.contains(&(*loc, *rot)) { e.1 += 1; }
    }
    let openings = [10, 13, 14, 17, 18, 19, 22, 25, 26, 29, 30, 31, 32];
    for (p, (n, with_portal)) in by_piece {
        let name = arcanum_formats::art_id::WALL_PIECES.get(p as usize).unwrap_or(&"?");
        if !openings.contains(&p) {
            assert_eq!(with_portal, 0, "piece {p} {name}: {n} walls");
        }
    }
}

/// Every NPC in every map (art its own or its prototype's) has art that exists, whether
/// critter, monster, unique NPC or scenery (or, for two bodies with no standing art, walk art).
#[test]
fn npc_art_resolves() {
    use arcanum_formats::art_id::ArtKind;
    use arcanum_formats::object::{Prototypes, TYPE_NPC};
    let (Some(archive), Some(vfs)) = (module_archive(), vfs()) else { return };
    let index = art_index(&vfs);
    let mut protos = Prototypes::default();
    for e in std::fs::read_dir(game_dir().join("data/proto")).into_iter().flatten().flatten() {
        protos.insert(Object::parse_prototype(&std::fs::read(e.path()).unwrap()).unwrap());
    }
    let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
    let mut missing: BTreeMap<String, usize> = BTreeMap::new();
    for path in map_files(&archive, ".mob") {
        let obj = Object::parse_instance(&archive.read(&path).unwrap()).unwrap();
        if obj.object_type != TYPE_NPC {
            continue;
        }
        let Some(art) = protos.art_id(&obj) else {
            *missing.entry("no art id".into()).or_default() += 1;
            continue;
        };
        *kinds.entry(format!("{:?}", art.kind())).or_default() += 1;
        // Two bodies have no standing art (`dgnuw`, `air`): the client then holds the walk art.
        let walk = ArtId((art.0 & !(0x1f << 6)) | 1 << 6);
        match index.path(art) {
            Some(p) if vfs.exists(&p) => {}
            Some(p) if matches!(art.kind(), ArtKind::Monster | ArtKind::UniqueNpc) && index.path(walk).is_some_and(|w| vfs.exists(&w)) => {
                *kinds.entry(format!("walk instead of {p}")).or_default() += 1;
            }
            Some(p) => *missing.entry(p).or_default() += 1,
            None => *missing.entry(format!("{:?} {:#x}: no path", art.kind(), art.0)).or_default() += 1,
        }
        assert!(matches!(art.kind(), ArtKind::Critter | ArtKind::Monster | ArtKind::UniqueNpc | ArtKind::Scenery), "{path}: {:?}", art.kind());
    }
    eprintln!("NPC art kinds: {kinds:?}; missing: {missing:?}");
    assert!(missing.is_empty(), "{missing:?}");
}

/// Every container and item in every map's `.mob` files (art its own or its prototype's) has
/// art that exists.
#[test]
fn container_and_item_art_resolves() {
    use arcanum_formats::object::{Prototypes, TYPE_CONTAINER, TYPE_GENERIC, TYPE_WEAPON};
    let (Some(archive), Some(vfs)) = (module_archive(), vfs()) else { return };
    let index = art_index(&vfs);
    let mut protos = Prototypes::default();
    for e in std::fs::read_dir(game_dir().join("data/proto")).into_iter().flatten().flatten() {
        protos.insert(Object::parse_prototype(&std::fs::read(e.path()).unwrap()).unwrap());
    }
    let (mut found, mut missing) = (BTreeMap::<String, usize>::new(), BTreeMap::<String, usize>::new());
    let mut exists: HashMap<String, bool> = HashMap::new();
    for path in map_files(&archive, ".mob") {
        let obj = Object::parse_instance(&archive.read(&path).unwrap()).unwrap();
        if obj.object_type != TYPE_CONTAINER && !(TYPE_WEAPON..=TYPE_GENERIC).contains(&obj.object_type) {
            continue;
        }
        let Some(art) = protos.art_id(&obj) else {
            *missing.entry("no art id".into()).or_default() += 1;
            continue;
        };
        let kind = format!("{:?}", art.kind());
        match index.path(art) {
            Some(p) if *exists.entry(p.clone()).or_insert_with(|| vfs.exists(&p)) => *found.entry(kind).or_default() += 1,
            Some(p) => *missing.entry(p).or_default() += 1,
            None => *missing.entry(format!("{kind} {:#x}: no path", art.0)).or_default() += 1,
        }
    }
    eprintln!("found by art kind: {found:?}; missing: {missing:?}");
    assert!(missing.is_empty(), "{} missing: {missing:?}", missing.values().sum::<usize>());
}
