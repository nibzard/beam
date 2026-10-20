// ABOUTME: Transfer animation in a strip of rows above the progress line during real transfers.
// ABOUTME: It never takes over the screen: other output prints above the strip, and the strip is erased at the end.
use crate::{
    raster::{Canvas, Cell, Encoder},
    scene,
};
use std::io::Write;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Scene rows, then one row for the destination names.
const SCENE_ROWS: usize = 9;
const ROWS: usize = SCENE_ROWS + 1;
const MAX_WIDTH: usize = 72;
const FRAME: Duration = Duration::from_millis(33);
const GRAY: [u8; 3] = [110, 120, 130];

/// While a leg of the work runs, progress moves toward the end of the leg, but covers only
/// `CREEP` of it: the next real event completes the leg. `PACE` is the time constant in seconds.
const CREEP: f32 = 0.6;
const PACE: f32 = 4.0;
/// The shown progress follows its goal with this time constant in seconds. A completed leg thus
/// shows in a fraction of a second, and no work waits for the animation.
const CHASE: f32 = 0.25;

struct Strip {
    width: usize,
    canvas: Canvas,
    encoder: Encoder,
    cells: Vec<Cell>,
    down: bool,
    started: Instant,
    leg: Leg,
    shown: f32,
    painted: Instant,
}

/// The part of the outward timeline that the current work moves through.
#[derive(Clone, Copy)]
struct Leg {
    from: f32,
    to: f32,
    started: Instant,
}

static STRIP: Mutex<Option<Strip>> = Mutex::new(None);

/// The outward progress that a leg has earned after `seconds` of work.
fn goal(from: f32, to: f32, seconds: f32) -> f32 {
    from + (to - from) * CREEP * (1.0 - (-seconds / PACE).exp())
}

/// Move `shown` toward `goal` for a frame of `seconds`.
fn chase(shown: f32, goal: f32, seconds: f32) -> f32 {
    goal + (shown - goal) * (-seconds / CHASE).exp()
}

fn strip_width(columns: u16) -> usize {
    usize::from(columns.saturating_sub(2)).min(MAX_WIDTH)
}

impl Strip {
    fn new(width: usize, down: bool) -> Self {
        Self {
            width,
            canvas: Canvas::new(width, SCENE_ROWS),
            encoder: Encoder::new(width, ROWS, crate::ui::truecolor()),
            cells: vec![Cell::default(); width * ROWS],
            down,
            started: Instant::now(),
            leg: Leg {
                from: 0.0,
                to: 0.0,
                started: Instant::now(),
            },
            shown: 0.0,
            painted: Instant::now(),
        }
    }

    /// Advance the shown progress to this frame and return the timeline position.
    fn progress(&mut self) -> f32 {
        let leg = self.leg;
        let target = goal(leg.from, leg.to, leg.started.elapsed().as_secs_f32());
        self.shown = chase(self.shown, target, self.painted.elapsed().as_secs_f32());
        self.painted = Instant::now();
        if self.down {
            1.0 - self.shown
        } else {
            self.shown
        }
    }

    /// Complete the current leg and start one that ends at `to`.
    fn begin(&mut self, to: f32) {
        // Before the departure nothing moves, so a leg that goes beyond it starts there.
        let from = self.leg.to.max(self.shown).max(scene::DEPART.min(to));
        self.leg = Leg {
            from,
            to: to.max(from),
            started: Instant::now(),
        };
    }

    /// Render the scene and the destination names into `cells`.
    fn render(&mut self) {
        let width = self.width;
        let progress = self.progress();
        scene::render(
            &mut self.canvas,
            width,
            SCENE_ROWS,
            progress,
            self.started.elapsed().as_secs_f32(),
        );
        self.cells[..width * SCENE_ROWS].copy_from_slice(self.canvas.resolve());
        let names = &mut self.cells[width * SCENE_ROWS..];
        names.fill(Cell::default());
        // The scene puts the destinations at one fifth and four fifths of the width.
        for (name, center) in [("Your machine", width / 5), ("Sandbox", width * 4 / 5)] {
            let start = center.saturating_sub(name.len() / 2);
            for (cell, glyph) in names.iter_mut().skip(start).zip(name.chars()) {
                *cell = Cell { glyph, color: GRAY };
            }
        }
    }

    /// Draw the cells that changed since the last frame.
    fn draw(&mut self, out: &mut impl Write) {
        let mut output = String::new();
        self.encoder.encode_above(&self.cells, &mut output);
        if !output.is_empty() {
            let _ = write!(out, "\x1b[?2026h{output}\x1b[?2026l");
            let _ = out.flush();
        }
    }
}

/// Clear the strip rows and the cursor line, and leave the cursor at the top row of the strip.
/// The cursor shows again: it is hidden only while the strip is on the screen.
fn erase(out: &mut impl Write) {
    let _ = write!(out, "\r\x1b[{ROWS}A\x1b[J\x1b[?25h");
}

/// Reserve the strip rows below the cursor line. The cursor at the end of the progress line
/// would otherwise blink below the animation.
fn reserve(out: &mut impl Write) {
    let _ = out.write_all(&[b'\n'; ROWS]);
    let _ = out.write_all(b"\x1b[?25l");
}

/// True while a strip is on the screen.
pub fn active() -> bool {
    STRIP.lock().unwrap_or_else(|e| e.into_inner()).is_some()
}

/// Before a line prints: move to the top of the strip and clear it. Returns false without a strip.
pub fn lift(out: &mut impl Write) -> bool {
    if !active() {
        return false;
    }
    erase(out);
    true
}

/// After a line printed: reserve the strip rows again below it and draw the current frame.
pub fn lower(out: &mut impl Write) {
    let mut strip = STRIP.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(strip) = strip.as_mut() {
        reserve(out);
        strip.encoder.invalidate();
        strip.draw(out);
    }
}

/// Remove the strip after an interruption. The process exits after this.
pub fn interrupted() {
    // Release the strip lock before stdout: `say` holds stdout while it waits for the strip.
    let removed = STRIP
        .try_lock()
        .is_ok_and(|mut strip| strip.take().is_some());
    if removed {
        let mut out = std::io::stdout().lock();
        erase(&mut out);
        let _ = out.flush();
    }
}

/// The animation for one transfer. Dropping it erases the strip at once.
pub struct Show {
    stop: Arc<AtomicBool>,
    painter: Option<std::thread::JoinHandle<()>>,
}

impl Show {
    /// Start the strip, or return None when motion is off or the window is too small.
    pub fn start(down: bool) -> Option<Self> {
        let (columns, rows) = crossterm::terminal::size().ok()?;
        if !crate::ui::animation()
            || std::env::var("BEAM_EFFECT").is_ok_and(|v| v == "off")
            || columns < 40
            || usize::from(rows) < ROWS + 10
            || active()
        {
            return None;
        }
        crate::transporter::cleanup_on_interrupt();
        crate::ui::exclusive(|| {
            let mut out = std::io::stdout().lock();
            reserve(&mut out);
            let mut strip = Strip::new(strip_width(columns), down);
            strip.render();
            strip.draw(&mut out);
            *STRIP.lock().unwrap_or_else(|e| e.into_inner()) = Some(strip);
        });
        let stop = Arc::new(AtomicBool::new(false));
        let painter = {
            let stop = stop.clone();
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    paint();
                    std::thread::sleep(FRAME);
                }
            })
        };
        Some(Self {
            stop,
            painter: Some(painter),
        })
    }

    /// The work that starts now ends at `to` on the outward timeline (0 to 1). Reaching an
    /// earlier leg's end is implied. With 1, the workspace forms at the destination.
    pub fn toward(&self, to: f32) {
        if let Some(strip) = STRIP.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            strip.begin(to);
        }
    }

    /// The workspace is at the destination. It forms there while the remaining work runs.
    pub fn arrive(&self) {
        if let Some(strip) = STRIP.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            strip.leg = Leg {
                from: 1.0,
                to: 1.0,
                started: Instant::now(),
            };
        }
    }
}

/// One frame. A changed window width clears the strip and draws it again at the new width.
fn paint() {
    crate::ui::exclusive(|| {
        let mut guard = STRIP.lock().unwrap_or_else(|e| e.into_inner());
        let Some(strip) = guard.as_mut() else {
            return;
        };
        let mut out = std::io::stdout().lock();
        if let Ok((columns, _)) = crossterm::terminal::size()
            && strip_width(columns) != strip.width
            && columns >= 40
        {
            erase(&mut out);
            let _ = write!(out, "\x1b[{ROWS}B\x1b[?25l");
            let mut resized = Strip::new(strip_width(columns), strip.down);
            resized.started = strip.started;
            resized.leg = strip.leg;
            resized.shown = strip.shown;
            *strip = resized;
        }
        strip.render();
        strip.draw(&mut out);
    });
}

impl Drop for Show {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(painter) = self.painter.take() {
            let _ = painter.join();
        }
        crate::ui::exclusive(|| {
            if STRIP
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take()
                .is_some()
            {
                let mut out = std::io::stdout().lock();
                erase(&mut out);
                let _ = out.flush();
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_leg_moves_toward_its_end_and_only_the_next_event_completes_it() {
        assert_eq!(goal(0.2, 0.6, 0.0), 0.2);
        let mut previous = 0.2;
        for seconds in [0.5, 2.0, 8.0, 60.0, 3600.0] {
            let progress = goal(0.2, 0.6, seconds);
            assert!(
                progress > previous && progress < 0.6,
                "{seconds}: {progress}"
            );
            previous = progress;
        }
        assert!((goal(0.2, 0.6, 3600.0) - (0.2 + 0.4 * CREEP)).abs() < 0.001);
        // The shown progress follows a jump within a fraction of a second and does not overshoot.
        assert_eq!(chase(0.3, 1.0, 0.0), 0.3);
        assert!(chase(0.3, 1.0, 0.75) > 0.95 && chase(0.3, 1.0, 0.75) < 1.0);
        assert!(chase(0.3, 1.0, 60.0) <= 1.0);
    }

    #[test]
    fn legs_start_at_the_departure_and_never_go_back() {
        let mut strip = Strip::new(60, false);
        strip.begin(0.6);
        assert_eq!(strip.leg.from, scene::DEPART);
        strip.shown = 0.4;
        strip.begin(1.0);
        assert_eq!((strip.leg.from, strip.leg.to), (0.6, 1.0));
        strip.shown = 0.9;
        strip.begin(0.6);
        assert_eq!((strip.leg.from, strip.leg.to), (1.0, 1.0));
    }

    #[test]
    fn the_cursor_is_hidden_only_while_the_strip_is_on_the_screen() {
        let mut out = Vec::new();
        reserve(&mut out);
        assert!(out.ends_with(b"\x1b[?25l"));
        out.clear();
        erase(&mut out);
        assert!(out.ends_with(b"\x1b[?25h"));
    }

    #[test]
    fn strip_shows_both_destination_names_and_the_workspace() {
        let mut strip = Strip::new(60, false);
        strip.render();
        let names: String = strip.cells[60 * SCENE_ROWS..]
            .iter()
            .map(|c| c.glyph)
            .collect();
        assert!(
            names.contains("Your machine") && names.contains("Sandbox"),
            "{names:?}"
        );
        assert!(
            strip.cells[..60 * SCENE_ROWS]
                .iter()
                .any(|c| c.glyph != ' ')
        );
        assert_eq!(strip_width(300), MAX_WIDTH);
        assert_eq!(strip_width(50), 48);
    }
}
