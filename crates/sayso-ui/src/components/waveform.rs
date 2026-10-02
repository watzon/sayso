//! Ink drawn with paths: the live waveform, history audio bars, the
//! processing line.

use gpui_kit::*;

/// Three layers of ink, from the wet edge to the dense core (design file).
const LAYERS: [(f32, f32); 3] = [(0.95, 0.08), (0.70, 0.18), (0.45, 1.0)];

/// The live ink waveform.
///
/// `levels` are recent input levels from 0.0 to 1.0, oldest first. The shape
/// tapers at both ends like a brush stroke. `phase` moves the ripple so the
/// ink looks alive between level updates.
pub fn ink_waveform(levels: Vec<f32>, phase: f32, color: Hsla) -> Canvas<()> {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            paint_ink(bounds, &levels, phase, color, window);
        },
    )
}

fn sample(levels: &[f32], x: f32) -> f32 {
    if levels.is_empty() {
        return 0.0;
    }
    let pos = x * (levels.len() - 1) as f32;
    let i = pos.floor() as usize;
    let j = (i + 1).min(levels.len() - 1);
    let t = pos - i as f32;
    levels[i] * (1.0 - t) + levels[j] * t
}

fn paint_ink(bounds: Bounds<Pixels>, levels: &[f32], phase: f32, color: Hsla, window: &mut Window) {
    const N: usize = 28;
    let w = bounds.size.width;
    let h = bounds.size.height;
    let mid = bounds.origin.y + h / 2.;
    let half = h / 2.;
    for (scale, alpha) in LAYERS {
        let amp = |i: usize| -> f32 {
            let x = i as f32 / (N - 1) as f32;
            // Brush taper: thin at both ends, full in the middle.
            let taper = (x * std::f32::consts::PI).sin().max(0.0).powf(0.8);
            let level = sample(levels, x).clamp(0.0, 1.0);
            let ripple = 0.82 + 0.18 * ((x * 13.0 + phase * 6.0).sin() * (x * 5.0 - phase * 2.0).cos());
            // A small floor so silence still shows a hairline of ink.
            (0.04 + level * 0.96) * taper * ripple * scale
        };
        let top: Vec<Point<Pixels>> = (0..N)
            .map(|i| point(bounds.origin.x + w * (i as f32 / (N - 1) as f32), mid - half * amp(i)))
            .collect();
        let bottom: Vec<Point<Pixels>> = top.iter().map(|p| point(p.x, mid + (mid - p.y))).collect();
        let mut pb = PathBuilder::fill();
        pb.move_to(top[0]);
        smooth_through(&mut pb, &top);
        pb.line_to(*bottom.last().unwrap());
        let rev: Vec<Point<Pixels>> = bottom.into_iter().rev().collect();
        smooth_through(&mut pb, &rev);
        pb.close();
        if let Ok(path) = pb.build() {
            window.paint_path(path, color.opacity(alpha));
        }
    }
}

/// Quadratic curves through the midpoints of `pts`: a smooth line.
fn smooth_through(pb: &mut PathBuilder, pts: &[Point<Pixels>]) {
    for i in 1..pts.len() - 1 {
        let m = point((pts[i].x + pts[i + 1].x) / 2., (pts[i].y + pts[i + 1].y) / 2.);
        pb.curve_to(m, pts[i]);
    }
    pb.line_to(*pts.last().unwrap());
}

/// History audio bars. Bars before `progress` (0.0 to 1.0) are played ink.
pub fn audio_bars(peaks: Vec<u8>, progress: f32, played: Hsla, rest: Hsla) -> Canvas<()> {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            if peaks.is_empty() {
                return;
            }
            let n = peaks.len() as f32;
            let gap = px(2.);
            let bar_w = (bounds.size.width - gap * (n - 1.)) / n;
            for (i, p) in peaks.iter().enumerate() {
                let a = (*p as f32 / 255.0).max(0.08);
                let bh = bounds.size.height * a;
                let x = bounds.origin.x + (bar_w + gap) * i as f32;
                let y = bounds.origin.y + (bounds.size.height - bh) / 2.;
                let color = if (i as f32 + 0.5) / n <= progress { played } else { rest };
                window.paint_quad(fill(Bounds { origin: point(x, y), size: size(bar_w, bh) }, color).corner_radii(px(1.4)));
            }
        },
    )
}

/// The processing state: a line of ink drawn across, with a wet nib.
pub fn ink_line(progress: f32, ink: Hsla, pencil: Hsla) -> Canvas<()> {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let y = bounds.origin.y + bounds.size.height / 2.;
            let x0 = bounds.origin.x + px(2.);
            let x1 = bounds.origin.x + bounds.size.width - px(2.);
            let nib = x0 + (x1 - x0) * progress.clamp(0.0, 1.0);
            let mut pb = PathBuilder::stroke(px(2.4));
            pb.move_to(point(x0, y));
            let mid = point((x0 + nib) / 2., y + px(0.6));
            pb.curve_to(point(nib, y), mid);
            if let Ok(path) = pb.build() {
                window.paint_path(path, ink);
            }
            // Dotted remainder.
            let mut x = nib + px(8.);
            while x < x1 {
                window.paint_quad(fill(Bounds { origin: point(x, y - px(0.7)), size: size(px(1.4), px(1.4)) }, pencil).corner_radii(px(0.7)));
                x += px(5.);
            }
            // The nib and its wet halo.
            let r = px(3.2);
            window.paint_quad(fill(Bounds { origin: point(nib - r * 2., y - r * 2.), size: size(r * 4., r * 4.) }, ink.opacity(0.08)).corner_radii(r * 2.));
            window.paint_quad(fill(Bounds { origin: point(nib - r, y - r), size: size(r * 2., r * 2.) }, ink).corner_radii(r));
        },
    )
}
