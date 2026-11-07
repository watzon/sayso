//! Motion for the overlay: the card eases to each new size instead of a jump.
//!
//! A [`Spring`] moves one value to its target. A [`Morph`] is a box whose
//! size and opacity follow the natural size of its content: the content is
//! measured as it draws, and the next frames move the box to that size. The
//! box clips the content, so a card that gets smaller cuts its old edges and
//! a card that gets larger opens around its content.

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

/// The time a spring needs for most of a move, in seconds. Shorter is stiffer.
const RESPONSE: f32 = 0.32;
/// Below 1 the spring goes a little past its target, and comes back.
const DAMPING: f32 = 0.82;
/// A spring at rest is this near its target and this slow.
const REST: f32 = 0.25;
/// The longest step. A frame after a long pause does not jump.
const MAX_STEP: f32 = 1. / 30.;
/// The steps of the integration, for a stable spring at a low frame rate.
const SUBSTEP: f32 = 1. / 240.;

/// One value that moves to its target like a damped spring.
#[derive(Debug, Clone, Copy)]
pub struct Spring {
    value: f32,
    velocity: f32,
    target: f32,
    /// The value at rest is `target * scale`; see [`Spring::rest_scale`].
    rest: f32,
}

impl Spring {
    pub fn at(value: f32) -> Self {
        Spring { value, velocity: 0., target: value, rest: REST }
    }

    /// Make the rest threshold relative to a value range other than points.
    /// Opacity, for example, moves from 0 to 1 and needs a finer threshold.
    pub fn rest_scale(mut self, scale: f32) -> Self {
        self.rest = REST * scale;
        self
    }

    pub fn value(&self) -> f32 {
        self.value
    }

    pub fn target(&self) -> f32 {
        self.target
    }

    pub fn set_target(&mut self, target: f32) {
        self.target = target;
    }

    /// Go to the target at once, with no motion.
    pub fn snap(&mut self) {
        self.value = self.target;
        self.velocity = 0.;
    }

    pub fn moving(&self) -> bool {
        (self.value - self.target).abs() > self.rest || self.velocity.abs() > self.rest * 4.
    }

    /// Move the spring forward by `dt` seconds.
    pub fn step(&mut self, dt: f32) {
        if !self.moving() {
            self.snap();
            return;
        }
        let omega = std::f32::consts::TAU / RESPONSE;
        let (k, c) = (omega * omega, 2. * DAMPING * omega);
        let mut left = dt.clamp(0., MAX_STEP);
        while left > 0. {
            let h = left.min(SUBSTEP);
            let accel = -k * (self.value - self.target) - c * self.velocity;
            self.velocity += accel * h;
            self.value += self.velocity * h;
            left -= h;
        }
        if !self.moving() {
            self.snap();
        }
    }
}

/// The time between two frames of a motion.
#[derive(Debug, Clone, Copy, Default)]
pub struct Clock(Option<Instant>);

impl Clock {
    /// Seconds since the last tick, or zero at the first tick.
    pub fn tick(&mut self) -> f32 {
        let now = Instant::now();
        let dt = self.0.map_or(0., |t| now.duration_since(t).as_secs_f32());
        self.0 = Some(now);
        dt
    }
}

/// An opacity spring: it moves from 0 to 1, so it rests at a finer threshold.
pub fn opacity_spring() -> Spring {
    Spring::at(0.).rest_scale(0.004)
}

/// The springs of a [`Morph`], and what its content measured.
struct MorphState {
    width: Spring,
    height: Spring,
    opacity: Spring,
    /// The natural size of the content in the last frame. None before the first frame.
    natural: Option<Size<Pixels>>,
    present: bool,
    exit: Exit,
    clock: Clock,
}

/// How a [`Morph`] goes away, and how it comes back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    /// It shrinks to a small pill and fades: a card.
    Shrink,
    /// It keeps its width and folds to no height: a slip that feeds back in.
    Fold,
    /// It shrinks to a point and fades: a slip that goes back into the pill.
    Point,
}

/// Where the content sits in a [`Morph`] that is smaller than the content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    /// The box opens from its center.
    Center,
    /// The content follows the bottom edge of the box, so it slides out downward.
    Bottom,
}

/// A box that eases to the size of its content. Clones share one state.
#[derive(Clone)]
pub struct Morph(Rc<RefCell<MorphState>>);

/// The scale of a card that grows in or shrinks out: a small round dot,
/// a little lower than the card.
const SEED_HEIGHT: f32 = 0.7;

/// Space for the content to lay out in. The content wraps at this width,
/// the same width that the overlay window gives it.
const LAYER_WIDTH: f32 = super::WIDTH;
const LAYER_HEIGHT: f32 = super::HEIGHT;

impl Morph {
    pub fn new(exit: Exit) -> Self {
        Morph(Rc::new(RefCell::new(MorphState {
            width: Spring::at(0.),
            height: Spring::at(0.),
            opacity: opacity_spring(),
            natural: None,
            present: false,
            exit,
            clock: Clock::default(),
        })))
    }

    /// Show the content, or ease the box out. With `reduce`, every change is at once.
    /// Returns true while the box moves, so the caller asks for another frame.
    pub fn update(&self, present: bool, reduce: bool) -> bool {
        let mut guard = self.0.borrow_mut();
        let s = &mut *guard;
        let dt = s.clock.tick();
        let (exit, natural) = (s.exit, s.natural);
        // Grow in from the exit size: the first time, and after an exit that
        // ended. An exit that did not end turns back from where it is.
        if present && natural.is_some() && s.opacity.target() == 0. && s.opacity.value() < 0.05 {
            let (w, h) = seed(exit, natural);
            s.width = Spring::at(w);
            s.height = Spring::at(h);
        }
        s.present = present;
        let (w, h, o) = match (present, natural) {
            (true, Some(n)) => (n.width.as_f32(), n.height.as_f32(), 1.),
            // No measure yet: the first frame draws the content, and the next one opens.
            (true, None) => (0., 0., 0.),
            (false, n) => {
                let (w, h) = seed(exit, n);
                (w, h, 0.)
            }
        };
        s.width.set_target(w);
        s.height.set_target(h);
        s.opacity.set_target(o);
        for spring in [&mut s.width, &mut s.height, &mut s.opacity] {
            if reduce {
                spring.snap();
            } else {
                spring.step(dt);
            }
        }
        drop(guard);
        self.moving()
    }

    /// True while the box moves to its size, or fades.
    pub fn moving(&self) -> bool {
        let s = self.0.borrow();
        s.width.moving() || s.height.moving() || s.opacity.moving()
    }

    /// Change how the box goes away. The next exit, or the next entry, uses it.
    pub fn set_exit(&self, exit: Exit) {
        self.0.borrow_mut().exit = exit;
    }

    /// The height of the box in this frame, in points.
    pub fn height(&self) -> f32 {
        self.0.borrow().height.value().max(0.)
    }

    /// True when the box shows content in this frame (it can still move).
    pub fn present(&self) -> bool {
        self.0.borrow().present
    }

    /// True when the box is gone: absent and at rest.
    pub fn gone(&self) -> bool {
        let s = self.0.borrow();
        !s.present && !s.opacity.moving() && s.opacity.value() <= 0.01
    }

    pub fn opacity(&self) -> f32 {
        self.0.borrow().opacity.value().clamp(0., 1.)
    }

    /// The box with its content. `shell` gives the look of the box (fill,
    /// shadow, corners); the box gives it the size of this frame.
    pub fn render(&self, shell: Div, align: Align, content: impl IntoElement) -> Div {
        let (w, h, opacity) = {
            let s = self.0.borrow();
            (s.width.value().max(0.), s.height.value().max(0.), s.opacity.value().clamp(0., 1.))
        };
        let state = self.0.clone();
        let measure = canvas(
            move |b, window, _| {
                let mut s = state.borrow_mut();
                // Empty content (a card that goes away) keeps the last size, to shrink from.
                if b.size.width <= px(0.) || b.size.height <= px(0.) {
                    return;
                }
                let changed = s.natural.is_none_or(|n| (n.width - b.size.width).abs() > px(0.5) || (n.height - b.size.height).abs() > px(0.5));
                if changed {
                    s.natural = Some(b.size);
                    window.request_animation_frame();
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();
        let top = match align {
            Align::Center => (h - LAYER_HEIGHT) / 2.,
            Align::Bottom => h - LAYER_HEIGHT,
        };
        let layer = div()
            .absolute()
            .left(px((w - LAYER_WIDTH) / 2.))
            .top(px(top))
            .w(px(LAYER_WIDTH))
            .h(px(LAYER_HEIGHT))
            .flex()
            .flex_col()
            .items_center()
            .map(|d| match align {
                Align::Center => d.justify_center(),
                Align::Bottom => d.justify_end(),
            })
            .child(div().relative().flex_none().child(content).child(measure));
        shell.relative().flex_none().w(px(w)).h(px(h)).overflow_hidden().opacity(opacity).child(layer)
    }
}

/// The size that a box grows in from and goes out to.
fn seed(exit: Exit, natural: Option<Size<Pixels>>) -> (f32, f32) {
    let (w, h) = natural.map_or((0., 0.), |n| (n.width.as_f32(), n.height.as_f32()));
    match exit {
        Exit::Shrink => (h * SEED_HEIGHT, h * SEED_HEIGHT),
        Exit::Fold => (w, 0.),
        Exit::Point => (0., 0.),
    }
}

#[cfg(test)]
mod tests {
    use super::Spring;

    fn run(s: &mut Spring, seconds: f32) {
        let mut t = 0.;
        while t < seconds {
            s.step(1. / 60.);
            t += 1. / 60.;
        }
    }

    #[test]
    fn a_spring_comes_to_rest_on_its_target() {
        let mut s = Spring::at(52.);
        s.set_target(320.);
        run(&mut s, 1.5);
        assert!(!s.moving());
        assert_eq!(s.value(), 320.);
    }

    #[test]
    fn a_spring_moves_most_of_the_way_in_its_response_time() {
        let mut s = Spring::at(0.);
        s.set_target(100.);
        run(&mut s, 0.25);
        assert!(s.value() > 80., "{}", s.value());
    }

    #[test]
    fn a_new_target_keeps_the_motion_smooth() {
        let mut s = Spring::at(0.);
        s.set_target(100.);
        run(&mut s, 0.1);
        let before = s.value();
        s.set_target(0.);
        s.step(1. / 60.);
        // The value goes on from where it was: no jump to the new target, and
        // the motion carries it on for a moment before it turns.
        assert!(s.value() >= before && s.value() - before < 10., "{before} -> {}", s.value());
    }

    #[test]
    fn a_long_pause_does_not_jump() {
        let mut s = Spring::at(0.);
        s.set_target(100.);
        s.step(5.);
        assert!(s.value() < 100., "{}", s.value());
    }
}
