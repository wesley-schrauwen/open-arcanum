//! Loads real game assets through Bevy's asset server, backed only by the VFS.
//!
//! The game folder comes from the resource pack: `ARCANUM_DIR` if set, otherwise
//! `assets/Arcanum` at the workspace root. Tests skip when no game data is there.
//! `tests/headless.rs` covers the same code with synthetic data.

use std::sync::{Arc, RwLock};

use arcanum_client::resource_pack::{DEFAULT_GAME_DIR, OVERRIDE_DIR_VAR, paths};
use arcanum_client::{ArcanumAssetsPlugin, MesAsset, ResourcePack, VfsAssetPlugin};
use arcanum_formats::test_util::{ArtBuilder, TestFrame};
use arcanum_vfs::Vfs;
use bevy::app::TaskPoolPlugin;
use bevy::asset::{AssetPlugin, LoadState};
use bevy::image::Image;
use bevy::prelude::*;

/// The resource pack from the environment, with the default game folder resolved against the
/// workspace root instead of the test's working directory.
fn pack() -> ResourcePack {
    let workspace_default = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
    ResourcePack::from_vars(
        // The override folder is supplied per test, never inherited.
        |name| (name != OVERRIDE_DIR_VAR).then(|| std::env::var(name).ok()).flatten(),
        std::path::Path::new(workspace_default).join(DEFAULT_GAME_DIR),
    )
}

/// The game's VFS, or `None` (and a note) when the game data is not installed.
fn game_vfs(pack: &ResourcePack) -> Option<Vfs> {
    match pack.open_vfs() {
        Ok(vfs) if vfs.exists(paths::backgrounds::MAIN_MENU) => Some(vfs),
        _ => {
            eprintln!("skipping: no game data in {} (set ARCANUM_DIR)", pack.game_dir.display());
            None
        }
    }
}

fn app(vfs: Vfs) -> App {
    let mut app = App::new();
    app.add_plugins((
        TaskPoolPlugin::default(),
        VfsAssetPlugin::new(Arc::new(RwLock::new(vfs))),
        AssetPlugin::default(),
    ))
    .init_asset::<Image>()
    .add_plugins(ArcanumAssetsPlugin);
    app
}

fn load<A: Asset>(app: &mut App, path: &'static str) -> Handle<A> {
    let handle: Handle<A> = app.world().resource::<AssetServer>().load(path);
    for _ in 0..2000 {
        app.update();
        match app.world().resource::<AssetServer>().load_state(&handle) {
            LoadState::Loaded => return handle,
            LoadState::Failed(e) => panic!("{path} failed to load: {e}"),
            _ => std::thread::sleep(std::time::Duration::from_millis(2)),
        }
    }
    panic!("timed out loading {path}");
}

#[test]
fn loads_main_menu_background() {
    let Some(vfs) = game_vfs(&pack()) else { return };
    let mut app = app(vfs);
    let handle = load::<Image>(&mut app, paths::backgrounds::MAIN_MENU);
    let image = app.world().resource::<Assets<Image>>().get(&handle).unwrap();
    assert_eq!((image.width(), image.height()), (800, 600));
}

#[test]
fn loads_main_menu_strings() {
    let Some(vfs) = game_vfs(&pack()) else { return };
    let mut app = app(vfs);
    let handle = load::<MesAsset>(&mut app, paths::text::MAIN_MENU);
    let mes = app.world().resource::<Assets<MesAsset>>().get(&handle).unwrap();
    for (id, text) in [(10, "Single Player"), (11, "MultiPlayer"), (12, "Options"), (13, "Credits"), (14, "Exit Game")] {
        assert_eq!(mes.get(id), Some(text));
    }
}

#[test]
fn override_folder_beats_the_game_archives() {
    let over = tempfile::tempdir().unwrap();
    let pack = ResourcePack { override_dir: Some(over.path().to_path_buf()), ..pack() };
    if game_vfs(&ResourcePack { override_dir: None, ..pack.clone() }).is_none() {
        return;
    }

    // A 1x1 sprite in the override folder replaces the archive's 800x600 background.
    let target = over.path().join(paths::backgrounds::MAIN_MENU);
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::write(&target, ArtBuilder::single(TestFrame::raw(1, 1, &[1])).build()).unwrap();

    let mut app = app(pack.open_vfs().unwrap());
    let handle = load::<Image>(&mut app, paths::backgrounds::MAIN_MENU);
    let image = app.world().resource::<Assets<Image>>().get(&handle).unwrap();
    assert_eq!((image.width(), image.height()), (1, 1));
}

#[test]
fn every_resource_pack_path_exists_in_the_game_data() {
    let Some(vfs) = game_vfs(&pack()) else { return };
    for file in paths::ALL_FILES {
        assert!(vfs.exists(file), "missing file {file}");
    }
    for dir in paths::ALL_DIRS {
        assert!(vfs.is_dir(dir), "missing directory {dir}");
        assert!(!vfs.list_dir(dir).unwrap().is_empty(), "empty directory {dir}");
    }
}

#[test]
fn pre_made_characters_load_with_their_story_and_portrait() {
    let Some(vfs) = game_vfs(&pack()) else { return };
    let premades = arcanum_client::premade::load(&vfs).unwrap();
    assert_eq!(premades.len(), 12);
    let lugard = premades.iter().find(|p| p.name == "Lugard Bloodstone").unwrap();
    assert_eq!((lugard.race.as_str(), lugard.gender.as_str()), ("Dwarf", "Male"));
    assert_eq!(lugard.background, 46);
    assert_eq!(arcanum_client::premade::split_story(&lugard.story).0, "Lugard Bloodstone");
    assert!(lugard.story.contains("steam-powered shovel"));
    assert_eq!(lugard.portrait.as_deref(), Some("portrait/DWM2_b.bmp"));
    for p in &premades {
        assert!(!p.name.is_empty() && !p.story.is_empty() && !p.race.is_empty(), "{p:?}");
        assert!(vfs.exists(p.portrait.as_deref().unwrap()), "{p:?}");
    }
    // The prototypes' real names, not the .mpc placeholders.
    assert!(premades.iter().any(|p| p.name == "Clarisse Vorak"));
}

#[test]
fn character_creation_rules_come_from_the_game_data() {
    use arcanum_rules::{FEMALE, MALE, RulesData};
    let Some(vfs) = game_vfs(&pack()) else { return };
    let rules = RulesData::load(&vfs).unwrap();
    assert_eq!(&rules.portraits_for(1, MALE)[..3], ["DWM1", "DWM2", "DWM3"]);
    assert!(rules.portraits_for(1, FEMALE).is_empty(), "no female dwarves");
    // Every allowed race and gender has portraits, and every listed portrait file exists.
    for race in 0..8u8 {
        for gender in [FEMALE, MALE] {
            if gender == FEMALE && !arcanum_rules::FEMALE_ALLOWED[race as usize] {
                continue;
            }
            let stems = rules.portraits_for(race, gender);
            assert!(!stems.is_empty(), "race {race} gender {gender}");
            for stem in stems {
                assert!(vfs.exists(&format!("portrait/{stem}_b.bmp")), "{stem}");
            }
        }
    }
    // "No significant background" first; the pre-made characters' own backgrounds are not offered.
    let human = rules.backgrounds_for(0, MALE);
    assert_eq!(human[0].description, 1000);
    assert!(human.len() > 10);
    assert!(rules.backgrounds.iter().filter(|b| (39..=50).contains(&b.id)).all(|b| b.npc));
    assert!(!human.iter().any(|b| b.npc));
    // Dwarf: st +1, cn +1, ch -1, dx -1 (rules/effect.mes 65).
    let dwarf = rules.step_race(&rules.new_sheet(), 1);
    assert_eq!(rules.stats(&dwarf), [9, 7, 9, 8, 8, 8, 8, 7]);
    // Women carry effect 330, "all females" (st -1, cn +1).
    let woman = rules.toggle_gender(&rules.new_sheet()).unwrap();
    assert_eq!(rules.stats(&woman), [7, 8, 9, 8, 8, 8, 8, 8]);
}

#[test]
fn every_background_loads_and_follows_its_race_gender_rules() {
    use arcanum_rules::{FEMALE, FEMALE_ALLOWED, MALE, RulesData, race_gender_code};
    use arcanum_formats::mes::MesFile;
    let Some(vfs) = game_vfs(&pack()) else { return };
    let rules = RulesData::load(&vfs).unwrap();
    let stories = MesFile::parse(&vfs.read("mes/gameback.mes").unwrap()).unwrap();
    let menu = MesFile::parse(&vfs.read(paths::text::MAIN_MENU).unwrap()).unwrap();
    let valid_codes: Vec<String> =
        (0..8u8).flat_map(|r| [race_gender_code(r, FEMALE), race_gender_code(r, MALE)]).chain(["ANY".to_string(), "NPC".to_string()]).collect();

    // Every offered background has a story with a title, and only known codes.
    for b in &rules.backgrounds {
        let story = stories.get(b.description).unwrap_or_else(|| panic!("background {} has no story {}", b.id, b.description));
        assert!(!story.lines().next().unwrap_or("").trim().is_empty(), "background {} has no title", b.id);
        for code in &b.codes {
            assert!(valid_codes.contains(code), "background {}: unknown code {code}", b.id);
        }
    }

    let mut combos = 0;
    let mut report = Vec::new();
    for race in 0..8u8 {
        for gender in [FEMALE, MALE] {
            if gender == FEMALE && !FEMALE_ALLOWED[race as usize] {
                continue;
            }
            combos += 1;
            let code = race_gender_code(race, gender);
            let offered = rules.backgrounds_for(race, gender);
            // Exactly the backgrounds whose codes allow this combination (or allow everyone).
            for b in &rules.backgrounds {
                let allowed = !b.npc && b.codes.is_empty() || !b.npc && b.codes.iter().any(|c| c == "ANY" || *c == code);
                assert_eq!(offered.iter().any(|o| o.id == b.id), allowed, "{code}: background {}", b.id);
            }
            assert_eq!(offered[0].description, 1000, "{code}: starts at no background");
            let restricted = offered.iter().filter(|b| !b.codes.is_empty()).count();
            report.push(format!(
                "{code} ({} {}): {} backgrounds, {restricted} restricted",
                menu.get(740 + u32::from(gender)).unwrap(),
                menu.get(720 + u32::from(race)).unwrap(),
                offered.len()
            ));
        }
    }
    assert_eq!(combos, 12, "the original's 12 race-gender combinations");
    let restricted_any = rules.backgrounds.iter().filter(|b| !b.npc && !b.codes.is_empty()).count();
    eprintln!("{} backgrounds offered in total, {restricted_any} restricted to some races/genders", rules.backgrounds.len());
    for line in report {
        eprintln!("{line}");
    }
}

#[test]
fn skill_tech_spell_and_schematic_tables_come_from_the_game_data() {
    use arcanum_rules::{DISCIPLINES, RulesData, SchematicSource};
    let Some(vfs) = game_vfs(&pack()) else { return };
    let rules = RulesData::load(&vfs).unwrap();
    assert_eq!(rules.skills.skills.len(), 16);
    assert!(rules.skills.skills.iter().all(|s| !s.name.is_empty()));
    assert_eq!(rules.skills.ranks, ["Apprentice", "Expert", "Master"]);
    assert_eq!(rules.tech.disciplines.len(), 8);
    assert_eq!((rules.tech.degrees[0].as_str(), rules.tech.degrees[7].as_str()), ("Layman", "Doctorate"));
    assert_eq!((rules.spells.spells.len(), rules.spells.colleges.len()), (80, 16));
    assert_eq!(rules.spells.colleges[0], "Conveyance");
    let learned: Vec<_> = rules
        .schematics
        .iter()
        .filter_map(|s| match s.source {
            SchematicSource::Learned { discipline, degree } => Some((discipline, degree)),
            SchematicSource::Found { .. } => None,
        })
        .collect();
    assert_eq!(learned.len(), 56);
    for d in 0..DISCIPLINES {
        let mut degrees: Vec<_> = learned.iter().filter(|(x, _)| *x == d).map(|(_, k)| *k).collect();
        degrees.sort();
        assert_eq!(degrees, [1, 2, 3, 4, 5, 6, 7], "discipline {d}");
    }
    assert_eq!(rules.schematics.len() - learned.len(), 70, "found schematics");
}

#[test]
fn premade_characters_become_sheets() {
    use arcanum_rules::{CharacterAction, RulesData};
    let Some(vfs) = game_vfs(&pack()) else { return };
    let rules = RulesData::load(&vfs).unwrap();
    assert_eq!(rules.premades.len(), 12);
    let lugard = rules.premades.iter().find(|p| p.prototype == 16094).unwrap();
    assert_eq!((lugard.sheet.name.as_str(), lugard.sheet.race, lugard.sheet.background), ("Lugard Bloodstone", 1, 46));
    assert!(rules.background(&lugard.sheet).is_some_and(|b| b.npc), "his own NPC background counts");
    // Inventory source 24, General(sm)PC: 385 gold, 6029, 8168, 8065 (Rules/InvenSource.mes).
    assert_eq!((lugard.gold, lugard.items.clone()), (385, vec![6029, 8168, 8065]));
    let merwin = rules.premades.iter().find(|p| p.prototype == 16087).unwrap();
    assert_eq!(merwin.items.last(), Some(&15178), "Merwin's own source 108");
    eprintln!("Lugard: base {:?}, stats {:?}, unspent {}", lugard.sheet.base, rules.stats(&lugard.sheet), lugard.sheet.unspent);
    let picked = rules.apply(&rules.new_sheet(), &CharacterAction::PickPremade(16094)).unwrap();
    assert_eq!(picked, lugard.sheet);
}
