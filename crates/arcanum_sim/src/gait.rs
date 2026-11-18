//! How fast a critter walks: shared by the host (which times each step) and the client (which
//! plays the walk animation at the same pace).
//!
//! The original moves a walking critter by its walk art's frame deltas (0x43f7f0 adds each new
//! frame's `delta_x`/`delta_y` to the object's offset), so speed is the art's distance per frame
//! times the frame rate. The distance differs by body (a human's walk loop covers about 47 pixels
//! diagonally, a dwarf's 30, a half-ogre's 61) and by direction. The frame rate is not the art
//! header's: 0x437990 takes it from the speed stat (stat 0xb) per animation, and 0x42ee90 turns it
//! into milliseconds per frame, `clamp(1000 / fps, 30, 800)`.
//!
//! A step's time here is its screen length over the walk's pixels per frame in its direction,
//! times the frame time. Where the original re-bases the offset and moves the critter to the
//! next tile is not traced; one tile's screen distance per step is assumed.

use arcanum_formats::art::ArtFile;
use arcanum_formats::art_id::{ANIM_STAND, ANIM_WALK, ArtId, CritterArt};
use arcanum_formats::map::TileLoc;
use arcanum_rules::FEMALE_ALLOWED;

/// Art body per race (table 0x5c5170): human, dwarf, elf, half-elf, gnome, halfling, half-orc,
/// half-ogre.
pub const ART_BODY: [u32; 8] = [0, 1, 4, 0, 2, 2, 0, 3];

/// A PC's art id: the race's body and the gender (male when the race has no female art), in
/// underwear, standing, facing the camera (rotation 4), as the new-PC prototype (0x40d952) and
/// the creation screen (0x441c70) build it.
pub fn pc_art(race: u8, male: bool) -> ArtId {
    let race = usize::from(race).min(ART_BODY.len() - 1);
    let male = male || !FEMALE_ALLOWED[race];
    CritterArt { male, body: ART_BODY[race], rotation: 4, anim: ANIM_STAND, ..Default::default() }.id()
}

/// `art` with another animation, frame 0.
pub fn with_anim(art: ArtId, anim: u32) -> ArtId {
    CritterArt { anim, frame: 0, ..CritterArt::from_id(art) }.id()
}

/// The walk animation of `art`.
pub fn walk_art(art: ArtId) -> ArtId {
    with_anim(art, ANIM_WALK)
}

/// Walk frames per second at speed 0, 8 and 30 (0x437990, animation 1).
const WALK_FPS: [i32; 3] = [6, 17, 30];
/// Added for dwarves and for gnomes and halflings (art bodies 1 and 2), whose strides are short.
const SHORT_BODY_FPS: i32 = 4;

/// Walk frames per second for a critter of art body `body` with speed stat `speed`. The values
/// at speed 0, 8 and 30 are the original's; in between they are joined linearly (inferred).
pub fn walk_fps(speed: i32, body: u32) -> i32 {
    let [slow, normal, fast] = WALK_FPS;
    let s = speed.clamp(0, 30);
    let fps = if s <= 8 { slow + (normal - slow) * s / 8 } else { normal + (fast - normal) * (s - 8) / 22 };
    fps + if matches!(body, 1 | 2) { SHORT_BODY_FPS } else { 0 }
}

/// Milliseconds per frame (0x42ee90).
pub fn frame_millis(fps: i32) -> u32 {
    (1000 / fps.max(1)).clamp(30, 800) as u32
}

/// The rotation that faces a step from `from` to the neighbouring tile `to`, or `None` if the
/// tiles are the same or not neighbours. Rotations go clockwise from screen-up (0) to up-left
/// (7), as the critter walk art's frame deltas move. A step's screen direction follows
/// `loc_to_xy` (0x4b8680): x + 1 moves 40 left and 20 down, y + 1 moves 40 right and 20 down.
pub fn rotation_of_step(from: TileLoc, to: TileLoc) -> Option<usize> {
    let dx = i64::from(to.x) - i64::from(from.x);
    let dy = i64::from(to.y) - i64::from(from.y);
    Some(match (dx, dy) {
        (-1, -1) => 0,
        (-1, 0) => 1,
        (-1, 1) => 2,
        (0, 1) => 3,
        (1, 1) => 4,
        (1, 0) => 5,
        (1, -1) => 6,
        (0, -1) => 7,
        _ => return None,
    })
}

/// Screen length of a step in each rotation: 40 up or down, 80 across, the rest one tile edge.
fn step_pixels(rotation: usize) -> f64 {
    match rotation {
        0 | 4 => 40.0,
        2 | 6 => 80.0,
        _ => (40.0f64 * 40.0 + 20.0 * 20.0).sqrt(),
    }
}

/// How a critter walks: its frame time and, per rotation, the screen pixels its walk art moves
/// per frame.
#[derive(bevy_ecs::component::Component, Debug, Clone, Copy, PartialEq)]
pub struct Gait {
    pub frame_millis: u32,
    pub pixels_per_frame: [f64; 8],
}

impl Gait {
    /// A human's walk (`hmmuwxab.art`, the same for every human and elf walk checked), for when
    /// the art cannot be read.
    pub const HUMAN_PIXELS: [f64; 8] = [3.0, 4.741, 6.0, 4.741, 3.0, 4.741, 6.0, 4.741];

    /// The gait of walk art `walk` (all frames of each rotation) at `fps`.
    pub fn from_art(walk: &ArtFile, fps: i32) -> Self {
        let mut pixels_per_frame = Self::HUMAN_PIXELS;
        for (rotation, frames) in walk.frames.iter().enumerate().take(8) {
            if frames.is_empty() {
                continue;
            }
            let (dx, dy) = frames.iter().fold((0i64, 0i64), |(x, y), f| (x + i64::from(f.delta_x), y + i64::from(f.delta_y)));
            let per_frame = ((dx * dx + dy * dy) as f64).sqrt() / frames.len() as f64;
            if per_frame > 0.0 {
                pixels_per_frame[rotation] = per_frame;
            }
        }
        Self { frame_millis: frame_millis(fps), pixels_per_frame }
    }

    /// A human's gait at `fps`.
    pub fn human(fps: i32) -> Self {
        Self { frame_millis: frame_millis(fps), pixels_per_frame: Self::HUMAN_PIXELS }
    }

    /// The gait for a critter with art `art` and speed `speed`, reading its walk art through
    /// `read` (a VFS); a human's if it cannot be read.
    pub fn load(art: ArtId, speed: i32, read: impl Fn(&str) -> Option<Vec<u8>>) -> Self {
        let c = CritterArt::from_id(art).normalised();
        let fps = walk_fps(speed, c.body);
        let walk = CritterArt::from_id(walk_art(art)).path();
        match read(&walk).and_then(|d| ArtFile::parse(&d).ok()) {
            Some(file) => Self::from_art(&file, fps),
            None => Self::human(fps),
        }
    }

    /// Seconds a step from `from` to its neighbour `to` takes (0 if they are not neighbours).
    pub fn step_seconds(&self, from: TileLoc, to: TileLoc) -> f64 {
        let Some(rotation) = rotation_of_step(from, to) else { return 0.0 };
        let frames = step_pixels(rotation) / self.pixels_per_frame[rotation];
        frames * f64::from(self.frame_millis) / 1000.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(x: u32, y: u32) -> TileLoc {
        TileLoc { x, y }
    }

    #[test]
    fn pc_art_follows_race_and_gender() {
        // As the pre-made prototypes store it.
        assert_eq!(pc_art(0, true), ArtId(0x2800_2000), "human man");
        assert_eq!(pc_art(1, true), ArtId(0x2900_2000), "dwarf");
        assert_eq!(pc_art(5, true), ArtId(0x2a00_2000), "halfling");
        assert_eq!(pc_art(4, true), ArtId(0x2a00_2000), "gnome");
        assert_eq!(pc_art(7, true), ArtId(0x2b00_2000), "half-ogre");
        assert_eq!(pc_art(2, true), ArtId(0x2c00_2000), "elf");
        assert_eq!(pc_art(0, false), ArtId(0x2000_2000), "human woman");
        assert_eq!(pc_art(2, false), ArtId(0x2400_2000), "elf woman");
        assert_eq!(pc_art(1, false), ArtId(0x2900_2000), "no dwarf women");
        assert_eq!(CritterArt::from_id(walk_art(pc_art(0, true))).path(), "art/critter/hmm/hmmuwxab.art");
    }

    #[test]
    fn walk_speed_comes_from_the_speed_stat() {
        assert_eq!(walk_fps(8, 0), 17);
        assert_eq!(walk_fps(0, 0), 6);
        assert_eq!(walk_fps(30, 0), 30);
        assert_eq!(walk_fps(99, 0), 30, "clamped");
        assert_eq!(walk_fps(8, 1), 21, "dwarf");
        assert_eq!(walk_fps(4, 0), 11, "between 0 and 8");
        assert_eq!(frame_millis(17), 58);
        assert_eq!(frame_millis(1), 800);
        assert_eq!(frame_millis(100), 30);
    }

    #[test]
    fn steps_face_their_screen_direction() {
        let at = t(100, 100);
        let to = |dx: i32, dy: i32| t((100 + dx) as u32, (100 + dy) as u32);
        let order = [(-1, -1), (-1, 0), (-1, 1), (0, 1), (1, 1), (1, 0), (1, -1), (0, -1)];
        for (rotation, (dx, dy)) in order.into_iter().enumerate() {
            assert_eq!(rotation_of_step(at, to(dx, dy)), Some(rotation), "step ({dx}, {dy})");
        }
        assert_eq!(rotation_of_step(at, at), None);
        assert_eq!(rotation_of_step(at, to(2, 0)), None);
    }

    #[test]
    fn a_human_walks_about_two_tiles_a_second() {
        let g = Gait::human(17);
        let edge = g.step_seconds(t(10, 10), t(9, 10));
        let across = g.step_seconds(t(10, 10), t(9, 11));
        assert!((edge - 0.547).abs() < 0.01, "{edge}");
        assert!((across - 0.773).abs() < 0.01, "{across}");
        assert_eq!(g.step_seconds(t(10, 10), t(12, 10)), 0.0);
    }

    #[test]
    fn the_gait_comes_from_the_walk_art() {
        use arcanum_formats::test_util::{ArtBuilder, TestFrame};
        // Every rotation moves (4, -4) in 2 frames.
        let b = ArtBuilder::animated(2, |_, f| TestFrame::raw(1, 1, &[1]).with_offsets((0, 0), if f == 0 { (4, 0) } else { (0, -4) }));
        let art = ArtFile::parse(&b.build()).unwrap();
        let g = Gait::from_art(&art, 10);
        assert_eq!(g.frame_millis, 100);
        assert!((g.pixels_per_frame[2] - 32f64.sqrt() / 2.0).abs() < 1e-9);
    }
}
