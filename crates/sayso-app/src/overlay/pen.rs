//! The pen of the live preview: it writes the words out on the slip.
//!
//! New letters do not show all at once. The pen writes them one by one at a
//! hand's pace, and each letter fades in like wet ink. The tentative words
//! are a pencil draft. When the speech model commits them, a second pen
//! traces over the draft, and each letter turns from pencil to ink.
//!
//! A letter that the pen has not written yet keeps its place in the text,
//! but it does not show. So the lines do not move while the pen writes.

use super::motion::Clock;
use super::needs_space;

/// Letters per second that the pen writes when it is not behind.
const WRITE_PACE: f32 = 42.;
/// The pen writes faster when the model sends much text at once: its speed
/// is the letters it has still to write, divided by this many seconds. So a
/// long burst starts fast and slows to the normal pace as it ends.
const WRITE_CATCH_UP: f32 = 0.35;
/// Letters per second that the ink traces over the pencil draft.
const INK_PACE: f32 = 32.;
const INK_CATCH_UP: f32 = 0.5;
/// Seconds for a letter to fade in after the pen writes it.
const WET: f32 = 0.14;
/// Seconds for a letter to turn from pencil to ink.
const INKING: f32 = 0.22;

/// One letter of the text, and when the pens got to it.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Letter {
    ch: char,
    /// When the pen wrote it, in seconds on the clock of the pen.
    written: Option<f32>,
    /// When the ink traced over it.
    inked: Option<f32>,
}

/// How one letter looks in this frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stroke {
    /// 0: not written yet. 1: written, and the ink is dry.
    pub opacity: f32,
    /// 0: pencil. 1: ink.
    pub ink: f32,
}

#[derive(Debug, Default)]
pub struct Pen {
    letters: Vec<Letter>,
    /// The first letters that are committed. The rest is the tentative tail.
    committed: usize,
    /// How far the pen has written, in letters. It is fractional, so a slow
    /// frame rate does not change the pace.
    written: f32,
    inked: f32,
    /// Seconds since the pen started.
    now: f32,
    clock: Clock,
}

impl Pen {
    /// Forget the text: the next text is written from its start.
    pub fn clear(&mut self) {
        self.letters.clear();
        self.committed = 0;
        self.written = 0.;
        self.inked = 0.;
    }

    /// Give the pen the text of this frame. Letters that did not change keep
    /// their ink; the pen goes back to the first letter that changed.
    pub fn set(&mut self, committed: &str, tentative: &str) {
        let mut text: Vec<char> = committed.chars().collect();
        let committed_len = text.len();
        if needs_space(committed, tentative) {
            text.push(' ');
        }
        text.extend(tentative.chars());
        let same = self.letters.iter().zip(&text).take_while(|(l, ch)| l.ch == **ch).count();
        if same == text.len() && same == self.letters.len() && committed_len == self.committed {
            return;
        }
        self.letters.truncate(same);
        self.letters.extend(text[same..].iter().map(|&ch| Letter { ch, written: None, inked: None }));
        // A letter that is no longer committed goes back to pencil.
        for l in &mut self.letters[committed_len.min(same)..] {
            l.inked = None;
        }
        self.committed = committed_len;
        self.written = self.written.min(same as f32);
        self.inked = self.inked.min(committed_len.min(same) as f32);
    }

    /// Move the pens on. With `reduce`, all of the text shows at once.
    /// Returns true while the pens move or the ink is wet.
    pub fn update(&mut self, reduce: bool) -> bool {
        let dt = self.clock.tick();
        if reduce {
            self.finish();
        } else {
            self.advance(dt);
        }
        self.moving()
    }

    fn advance(&mut self, dt: f32) {
        // A long pause (the first frame) does not write a burst of letters.
        let dt = dt.clamp(0., 1. / 20.);
        self.now += dt;
        let len = self.letters.len() as f32;
        let behind = len - self.written;
        self.written = (self.written + WRITE_PACE.max(behind / WRITE_CATCH_UP) * dt).min(len);
        let to_ink = (self.committed as f32).min(self.written);
        let behind = to_ink - self.inked;
        if behind > 0. {
            self.inked = (self.inked + INK_PACE.max(behind / INK_CATCH_UP) * dt).min(to_ink);
        }
        let now = self.now;
        let (written, inked) = (self.written as usize, self.inked as usize);
        for (i, l) in self.letters.iter_mut().enumerate() {
            if i < written && l.written.is_none() {
                l.written = Some(now);
            }
            if i < inked && l.inked.is_none() {
                l.inked = Some(now);
            }
        }
    }

    /// Write and ink all of the text, dry.
    fn finish(&mut self) {
        let dry = self.now - WET.max(INKING);
        self.written = self.letters.len() as f32;
        self.inked = self.committed as f32;
        let committed = self.committed;
        for (i, l) in self.letters.iter_mut().enumerate() {
            l.written = Some(l.written.map_or(dry, |t| t.min(dry)));
            if i < committed {
                l.inked = Some(l.inked.map_or(dry, |t| t.min(dry)));
            }
        }
    }

    /// True while a pen moves, or ink is still wet.
    pub fn moving(&self) -> bool {
        let wet = |t: Option<f32>, span: f32| t.is_some_and(|t| self.now - t < span);
        self.written < self.letters.len() as f32
            || self.inked < (self.committed as f32).min(self.written)
            || self.letters.iter().any(|l| wet(l.written, WET) || wet(l.inked, INKING))
    }

    /// True when the pen has written all of the text.
    pub fn caught_up(&self) -> bool {
        self.written >= self.letters.len() as f32
    }

    pub fn letters(&self) -> impl Iterator<Item = char> + '_ {
        self.letters.iter().map(|l| l.ch)
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.letters.len()
    }

    pub fn committed(&self) -> usize {
        self.committed
    }

    /// How letter `i` looks in this frame.
    pub fn stroke(&self, i: usize) -> Stroke {
        let Some(l) = self.letters.get(i) else { return Stroke { opacity: 0., ink: 0. } };
        let ease = |t: Option<f32>, span: f32| t.map_or(0., |t| smooth(((self.now - t) / span).clamp(0., 1.)));
        Stroke { opacity: ease(l.written, WET), ink: ease(l.inked, INKING) }
    }
}

/// An ease in and out, so a stroke starts and ends softly.
fn smooth(t: f32) -> f32 {
    t * t * (3. - 2. * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(pen: &mut Pen, seconds: f32) {
        let mut t = 0.;
        while t < seconds {
            pen.advance(1. / 60.);
            t += 1. / 60.;
        }
    }

    fn shown(pen: &Pen) -> String {
        (0..pen.len()).filter(|&i| pen.stroke(i).opacity > 0.).map(|i| pen.letters().nth(i).unwrap()).collect()
    }

    #[test]
    fn the_pen_writes_new_letters_one_by_one() {
        let mut pen = Pen::default();
        pen.set("", "hello there");
        run(&mut pen, 0.1);
        let part = shown(&pen);
        assert!(!part.is_empty() && part.len() < "hello there".len(), "{part:?}");
        assert!("hello there".starts_with(&part));
        run(&mut pen, 1.);
        assert_eq!(shown(&pen), "hello there");
        assert!(!pen.moving());
        assert_eq!(pen.stroke(0).opacity, 1.);
    }

    #[test]
    fn the_pen_catches_up_when_much_text_comes_at_once() {
        let mut pen = Pen::default();
        // 200 letters take almost 5 seconds at the normal pace.
        pen.set(&"word ".repeat(40), "");
        run(&mut pen, 1.5);
        assert!(pen.caught_up());
    }

    #[test]
    fn a_revision_keeps_the_letters_before_it() {
        let mut pen = Pen::default();
        pen.set("", "I went their");
        run(&mut pen, 1.);
        pen.set("", "I went there");
        // "I went the" stays written; only the changed end is written again.
        assert_eq!(shown(&pen), "I went the");
        run(&mut pen, 1.);
        assert_eq!(shown(&pen), "I went there");
    }

    #[test]
    fn committed_words_turn_from_pencil_to_ink() {
        let mut pen = Pen::default();
        pen.set("", "good morning");
        run(&mut pen, 1.);
        assert_eq!(pen.stroke(0).ink, 0.);
        pen.set("good morning", "");
        run(&mut pen, 0.05);
        // The ink starts at the first letter and goes on from there.
        assert!(pen.stroke(0).ink > 0. && pen.stroke(11).ink == 0.);
        run(&mut pen, 1.);
        assert!((0..pen.len()).all(|i| pen.stroke(i).ink == 1.));
        // The written letters stay written while they are inked.
        assert_eq!(shown(&pen), "good morning");
    }

    #[test]
    fn a_word_that_is_no_longer_committed_goes_back_to_pencil() {
        let mut pen = Pen::default();
        pen.set("one two", "");
        run(&mut pen, 1.);
        pen.set("one", "two");
        assert_eq!(pen.stroke(0).ink, 1.);
        assert_eq!(pen.stroke(4).ink, 0.);
    }

    #[test]
    fn reduce_motion_shows_all_of_the_text_at_once() {
        let mut pen = Pen::default();
        pen.set("good", "morning");
        pen.update(true);
        assert_eq!(shown(&pen), "good morning");
        assert_eq!(pen.stroke(0), Stroke { opacity: 1., ink: 1. });
        assert_eq!(pen.stroke(6), Stroke { opacity: 1., ink: 0. });
        assert!(!pen.moving());
    }
}
