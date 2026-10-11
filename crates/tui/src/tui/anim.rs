//! Motion (addendum 13): easing, colour blends and the clock that tells the
//! event loop when to wake for a frame.
//!
//! Every position comes from an `Instant`, never a frame count (RAT-30), so a
//! late frame catches up instead of running slow.

use std::io::Write;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{BeginSynchronizedUpdate, EndSynchronizedUpdate};
use ratatui::style::Color;

/// How long a workspace switch takes (REQ-136).
pub const SWITCH: Duration = Duration::from_millis(240);

/// Time between two steps of the agent's spinner (REQ-143).
pub const SPIN_EVERY: Duration = Duration::from_millis(80);

/// A picker's highlighted row slides to the new one in this time (REQ-144).
pub const PICK_SLIDE: Duration = Duration::from_millis(120);

/// A modal, picker, settings screen or which-key popup fades in over this
/// (REQ-144).
pub const POPUP_FADE: Duration = Duration::from_millis(160);

/// The focused pane's border changes colour in this time (REQ-145).
pub const FOCUS_EASE: Duration = Duration::from_millis(180);

/// A turn of the wheel is spread over this (REQ-145).
pub const SCROLL: Duration = Duration::from_millis(120);

/// The welcome shows a step every this long (REQ-146).
pub const WELCOME_STEP: Duration = Duration::from_millis(650);

/// Each welcome step fades in over this.
pub const WELCOME_FADE: Duration = Duration::from_millis(250);

/// Cells the panes travel while they enter (REQ-136).
pub const SLIDE_CELLS: u16 = 4;

/// The mode chip's colour eases over this (REQ-140).
pub const CHIP_EASE: Duration = Duration::from_millis(180);

/// The old hint fades out and the new one fades in, in this time (REQ-140).
pub const HINT_FADE: Duration = Duration::from_millis(160);

/// A status message enters and leaves in this time each (REQ-142).
pub const MESSAGE_EDGE: Duration = Duration::from_millis(200);

/// Cells a status message travels while it enters (REQ-142).
pub const MESSAGE_CELLS: u16 = 8;

const BRAILLE: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// The spinner glyph `elapsed` after the TUI started.
pub fn spinner(elapsed: Duration) -> char {
    let step = elapsed.as_millis() / SPIN_EVERY.as_millis();
    BRAILLE[step as usize % BRAILLE.len()]
}

/// `elapsed` of `total`, 0 to 1.
pub fn progress(elapsed: Duration, total: Duration) -> f32 {
    if total.is_zero() {
        return 1.0;
    }
    (elapsed.as_secs_f32() / total.as_secs_f32()).clamp(0.0, 1.0)
}

/// The hint during a change of mode: whether the new one is showing, and how
/// visible whichever shows is. The old hint fades out in the first half, the
/// new one in during the second (REQ-140).
pub fn crossfade(elapsed: Duration) -> (bool, f32) {
    let t = progress(elapsed, HINT_FADE);
    if t < 0.5 {
        (false, 1.0 - 2.0 * t)
    } else {
        (true, 2.0 * t - 1.0)
    }
}

/// A status message `elapsed` after it appeared, of `total` on screen: how
/// visible it is, and how far along its entrance (REQ-142).
pub fn message(elapsed: Duration, total: Duration) -> (f32, f32) {
    let entered = ease_out(progress(elapsed, MESSAGE_EDGE));
    let leaving = progress(
        elapsed.saturating_sub(total.saturating_sub(MESSAGE_EDGE)),
        MESSAGE_EDGE,
    );
    (entered.min(1.0 - ease_out(leaving)), entered)
}

/// Fast at the start, settling at the end: the curve every transition uses.
pub fn ease_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

/// A value moving from `from` to `to` over `dur`.
#[derive(Clone, Copy, Debug)]
pub struct Tween {
    from: f32,
    to: f32,
    start: Instant,
    dur: Duration,
}

impl Tween {
    pub fn new(from: f32, to: f32, dur: Duration, now: Instant) -> Self {
        Self {
            from,
            to,
            start: now,
            dur,
        }
    }

    /// A tween that is already where it ends.
    pub fn at(v: f32, now: Instant) -> Self {
        Self::new(v, v, Duration::ZERO, now)
    }

    /// How far along it is, 0 to 1.
    fn progress(&self, now: Instant) -> f32 {
        if self.dur.is_zero() {
            return 1.0;
        }
        let t = now.saturating_duration_since(self.start).as_secs_f32() / self.dur.as_secs_f32();
        t.clamp(0.0, 1.0)
    }

    pub fn value(&self, now: Instant) -> f32 {
        self.from + (self.to - self.from) * ease_out(self.progress(now))
    }

    pub fn done(&self, now: Instant) -> bool {
        self.progress(now) >= 1.0
    }

    /// Heads for `to` from what is on screen now, so an interrupted move
    /// does not jump (REQ-148).
    pub fn retarget(&mut self, to: f32, dur: Duration, now: Instant) {
        *self = Self::new(self.value(now), to, dur, now);
    }
}

/// `a` blended toward `b`, `t` of the way. Only true colours blend; any other
/// pair has no shades in between, so it holds `a` until the end.
pub fn lerp(a: Color, b: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    match (a, b) {
        (Color::Rgb(ar, ag, ab), Color::Rgb(br, bg, bb)) => {
            let mix =
                |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
            Color::Rgb(mix(ar, br), mix(ag, bg), mix(ab, bb))
        }
        _ if t >= 1.0 => b,
        _ => a,
    }
}

/// Whether anything is moving, for the event loop: it wakes every frame
/// while this is true and blocks when it is not.
#[derive(Default)]
pub struct Motion {
    until: Option<Instant>,
}

impl Motion {
    /// Something that lasts `dur` starts now.
    pub fn kick(&mut self, dur: Duration, now: Instant) {
        let end = now + dur;
        if self.until.is_none_or(|u| end > u) {
            self.until = Some(end);
        }
    }

    pub fn active(&self, now: Instant) -> bool {
        self.until.is_some_and(|u| now < u)
    }
}

/// Runs `draw` between `CSI ? 2026 h` and `CSI ? 2026 l`, so the terminal
/// paints what it wrote whole or not at all (REQ-139). The frame closes
/// whatever `draw` returns.
pub fn synced<W: Write, R>(out: &mut W, draw: impl FnOnce() -> R) -> R {
    let _ = super::trace::span("sync_begin", || execute!(out, BeginSynchronizedUpdate));
    let r = draw();
    let _ = super::trace::span("sync_end", || execute!(out, EndSynchronizedUpdate));
    r
}

/// REQ-138: `animations` off in the settings stops every transition.
pub fn enabled() -> bool {
    crate::settings::get().animations()
}

/// Colour fades and slides need true colour (RISK-42).
pub fn truecolor() -> bool {
    static TRUECOLOR: OnceLock<bool> = OnceLock::new();
    *TRUECOLOR.get_or_init(|| {
        matches!(
            std::env::var("COLORTERM").as_deref(),
            Ok("truecolor" | "24bit")
        )
    })
}

/// Fades and slides run: animations on and a terminal that can blend.
pub fn fades() -> bool {
    enabled() && truecolor()
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: fn(u64) -> Duration = Duration::from_millis;

    #[test]
    fn ease_runs_from_zero_to_one_and_never_backs_up() {
        assert_eq!(ease_out(0.0), 0.0);
        assert_eq!(ease_out(1.0), 1.0);
        assert_eq!(ease_out(-3.0), 0.0);
        assert_eq!(ease_out(9.0), 1.0);
        let mut last = 0.0;
        for i in 0..=100 {
            let v = ease_out(i as f32 / 100.0);
            assert!(v >= last);
            last = v;
        }
    }

    #[test]
    fn a_tween_reaches_its_end_at_its_duration() {
        let t0 = Instant::now();
        let t = Tween::new(2.0, 10.0, MS(200), t0);
        assert_eq!(t.value(t0), 2.0);
        assert!(!t.done(t0 + MS(100)));
        assert_eq!(t.value(t0 + MS(200)), 10.0);
        assert!(t.done(t0 + MS(200)));
        assert_eq!(t.value(t0 + MS(900)), 10.0);
    }

    #[test]
    fn a_zero_length_tween_is_done() {
        let t0 = Instant::now();
        let t = Tween::at(5.0, t0);
        assert!(t.done(t0));
        assert_eq!(t.value(t0), 5.0);
    }

    #[test]
    fn an_interrupted_tween_starts_from_where_it_was() {
        let t0 = Instant::now();
        let mut t = Tween::new(0.0, 10.0, MS(200), t0);
        let mid = t.value(t0 + MS(80));
        t.retarget(0.0, MS(200), t0 + MS(80));
        assert!((t.value(t0 + MS(80)) - mid).abs() < 1e-4);
        assert_eq!(t.value(t0 + MS(280)), 0.0);
    }

    #[test]
    fn colours_blend_only_in_true_colour() {
        let a = Color::Rgb(0, 0, 0);
        let b = Color::Rgb(100, 200, 50);
        assert_eq!(lerp(a, b, 0.0), a);
        assert_eq!(lerp(a, b, 1.0), b);
        assert_eq!(lerp(a, b, 0.5), Color::Rgb(50, 100, 25));
        assert_eq!(lerp(Color::Blue, Color::Red, 0.5), Color::Blue);
        assert_eq!(lerp(Color::Blue, Color::Red, 1.0), Color::Red);
    }

    #[test]
    fn the_hint_fades_out_then_in() {
        assert_eq!(crossfade(Duration::ZERO), (false, 1.0));
        assert_eq!(crossfade(MS(80)), (true, 0.0));
        assert_eq!(crossfade(MS(160)), (true, 1.0));
        assert_eq!(crossfade(MS(900)), (true, 1.0));
        let (new, a) = crossfade(MS(40));
        assert!(!new && a > 0.0 && a < 1.0);
    }

    #[test]
    fn a_message_enters_stays_and_fades_before_it_goes() {
        let total = Duration::from_secs(4);
        assert_eq!(message(Duration::ZERO, total), (0.0, 0.0));
        assert_eq!(message(MS(200), total), (1.0, 1.0));
        assert_eq!(message(MS(2000), total), (1.0, 1.0));
        let (v, _) = message(MS(3900), total);
        assert!(v > 0.0 && v < 1.0);
        assert_eq!(message(total, total).0, 0.0);
    }

    #[test]
    fn the_spinner_steps_every_80_ms_and_wraps() {
        assert_eq!(spinner(Duration::ZERO), '⠋');
        assert_eq!(spinner(MS(79)), '⠋');
        assert_eq!(spinner(MS(80)), '⠙');
        assert_eq!(spinner(MS(800)), '⠋');
    }

    #[test]
    fn a_frame_is_wrapped_in_a_synchronized_update_even_when_it_fails() {
        let mut out = Vec::new();
        let mut ran = false;
        let r: Result<(), &str> = synced(&mut out, || {
            ran = true;
            Err("no tty")
        });
        assert!(ran && r.is_err());
        assert_eq!(out, b"\x1b[?2026h\x1b[?2026l");
    }

    #[test]
    fn motion_is_idle_until_kicked_and_idle_again_after() {
        let t0 = Instant::now();
        let mut m = Motion::default();
        assert!(!m.active(t0));
        m.kick(MS(100), t0);
        assert!(m.active(t0 + MS(50)));
        m.kick(MS(20), t0 + MS(10));
        assert!(
            m.active(t0 + MS(99)),
            "a shorter kick does not cut it short"
        );
        assert!(!m.active(t0 + MS(100)));
    }
}
