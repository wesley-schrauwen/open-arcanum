//! `.art` animation playback: frame timing and facing.
//!
//! An `.art` file stores its playback speed in frames per second (header offset 0x04,
//! [`ArtFrames::frame_rate`]). [`FrameClock`] steps through a rotation's frames at that rate, or
//! at a frame time of its own: the original times critter walks from the speed stat instead
//! ([`arcanum_sim::Gait`]). [`ArtAnimation`] puts the current frame of a rotation on a sprite,
//! anchored on the frame's hotspot.
//!
//! Rotations go clockwise from screen-up (0) to up-left (7) ([`arcanum_sim::rotation_of_step`]).

use bevy::image::{ImageFilterMode, ImageSampler};
use bevy::prelude::*;

use crate::art_loader::{ArtFrames, hotspot_anchor};

pub struct AnimationPlugin;

impl Plugin for AnimationPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, play_animations);
    }
}

/// Where an animation is in its frames.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct FrameClock {
    pub frame: usize,
    /// Time since `frame` started, in seconds.
    pub elapsed: f32,
}

impl FrameClock {
    /// Moves on by `dt` seconds, `period` seconds a frame, through `count` frames, wrapping to
    /// frame 0 after the last. Returns how many frames were stepped. A period of 0 or a single
    /// frame holds the current frame.
    pub fn advance(&mut self, dt: f32, period: f32, count: usize) -> usize {
        if period <= 0.0 || count <= 1 {
            self.frame = self.frame.min(count.saturating_sub(1));
            return 0;
        }
        self.elapsed += dt;
        let mut stepped = 0;
        while self.elapsed >= period {
            self.elapsed -= period;
            self.frame = (self.frame + 1) % count;
            stepped += 1;
        }
        stepped
    }
}

/// A sprite showing an `.art` file's frames in turn.
#[derive(Component, Debug, Clone)]
pub struct ArtAnimation {
    pub art: Handle<ArtFrames>,
    pub rotation: usize,
    pub clock: FrameClock,
    /// `false` holds the current frame.
    pub playing: bool,
    /// Milliseconds a frame, instead of the file's frame rate.
    pub frame_millis: Option<u32>,
}

impl ArtAnimation {
    pub fn new(art: Handle<ArtFrames>, rotation: usize) -> Self {
        Self { art, rotation, clock: FrameClock::default(), playing: true, frame_millis: None }
    }

    /// Switches to other art (or rotation), from its first frame. Nothing changes if it is the
    /// same art and rotation.
    pub fn set(&mut self, art: &Handle<ArtFrames>, rotation: usize) {
        if self.art != *art {
            self.art = art.clone();
            self.clock = FrameClock::default();
        }
        self.rotation = rotation;
    }
}

/// Steps every playing [`ArtAnimation`] and shows its current frame, with nearest sampling (as
/// the world's other sprites: linear filtering bleeds the transparent colour, blue in critter
/// palettes, into the edges).
fn play_animations(
    time: Res<Time>,
    frames: Res<Assets<ArtFrames>>,
    mut images: ResMut<Assets<Image>>,
    mut query: Query<(&mut ArtAnimation, &mut Sprite, &mut bevy::sprite::Anchor)>,
) {
    let dt = time.delta_secs();
    for (mut animation, mut sprite, mut anchor) in &mut query {
        let Some(art) = frames.get(&animation.art) else { continue };
        let Some(rotation) = art.rotation(animation.rotation) else { continue };
        let count = rotation.frames.len();
        let period = match animation.frame_millis {
            Some(ms) => ms as f32 / 1000.0,
            None if art.frame_rate > 0 => 1.0 / art.frame_rate as f32,
            None => 0.0,
        };
        if animation.playing {
            animation.clock.advance(dt, period, count);
        }
        let Some((image, center, size)) = rotation.frame(animation.clock.frame) else { continue };
        if sprite.image != *image {
            if images.get(image).is_some_and(|i| !matches!(i.sampler, ImageSampler::Descriptor(ref d) if d.mag_filter == ImageFilterMode::Nearest))
                && let Some(mut i) = images.get_mut(image)
            {
                i.sampler = ImageSampler::nearest();
            }
            sprite.image = image.clone();
        }
        let new = hotspot_anchor(center, size);
        if *anchor != new {
            *anchor = new;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_clock_steps_at_the_frame_rate_and_wraps() {
        let mut c = FrameClock::default();
        assert_eq!(c.advance(0.05, 0.1, 4), 0, "half a frame");
        assert_eq!(c.advance(0.05, 0.1, 4), 1);
        assert_eq!(c.frame, 1);
        assert_eq!(c.advance(0.35, 0.1, 4), 3, "several frames in one update");
        assert_eq!(c.frame, 0, "wrapped after the last");
        assert!(c.elapsed < 0.1);
    }

    #[test]
    fn the_clock_holds_still_art() {
        let mut c = FrameClock { frame: 5, elapsed: 0.0 };
        assert_eq!(c.advance(1.0, 0.0, 8), 0, "period 0");
        assert_eq!(c.frame, 5);
        assert_eq!(c.advance(1.0, 0.1, 1), 0, "one frame");
        assert_eq!(c.frame, 0, "clamped into range");
    }
}
