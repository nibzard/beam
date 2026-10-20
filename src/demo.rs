// ABOUTME: Explicit, bounded terminal preview with cancellation, resize handling, and frame measurements.
// ABOUTME: Uses the existing Crossterm terminal controls; Unix poll avoids adding input dependencies.
use crate::{
    raster::{Canvas, Encoder},
    scene, ui,
};
use anyhow::{Context, Result};
use std::io::{IsTerminal, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

static INTERRUPTED: AtomicBool = AtomicBool::new(false);
const FRAME: Duration = Duration::from_millis(33);

#[derive(Debug)]
pub struct Interrupted;
impl std::fmt::Display for Interrupted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Demo interrupted")
    }
}
impl std::error::Error for Interrupted {}

pub fn run(down: bool, seconds: u64, benchmark: bool) -> Result<()> {
    if benchmark {
        return measure(down);
    }
    if !ui::animation() || !std::io::stdin().is_terminal() {
        static_preview(down);
        return Ok(());
    }
    let size = crossterm::terminal::size().context("Cannot read the terminal size")?;
    if size.0 < 40 || size.1 < 12 {
        ui::say("The animated demo needs at least 40 columns and 12 rows.");
        static_preview(down);
        return Ok(());
    }
    #[cfg(unix)]
    {
        play(down, seconds, size)
    }
    #[cfg(not(unix))]
    {
        let _ = seconds;
        static_preview(down);
        Ok(())
    }
}

fn static_preview(down: bool) {
    ui::say("Beam demo: no files are transferred.");
    for stage in [0.0, 0.5, 1.0] {
        ui::step("demo", scene::stage(stage, down));
    }
    ui::say(
        "Run beam demo in an interactive terminal with animation enabled to see the text effect.",
    );
}

struct Screen;
impl Screen {
    fn enter() -> Result<Self> {
        crossterm::terminal::enable_raw_mode()?;
        let guard = Self;
        let mut out = std::io::stdout().lock();
        out.write_all(b"\x1b[?1049h\x1b[?25l\x1b[?7l\x1b[0m\x1b[2J")?;
        out.flush()?;
        Ok(guard)
    }
}
impl Drop for Screen {
    fn drop(&mut self) {
        let mut out = std::io::stdout().lock();
        let _ = out.write_all(b"\x1b[?2026l\x1b[0m\x1b[?7h\x1b[?25h\x1b[?1049l");
        let _ = out.flush();
        let _ = crossterm::terminal::disable_raw_mode();
    }
}

#[cfg(unix)]
fn play(down: bool, seconds: u64, mut size: (u16, u16)) -> Result<()> {
    use std::os::fd::AsFd;
    // Read through an unbuffered owned descriptor. Stdin's internal buffer can hide queued keys from poll.
    let mut keys = std::fs::File::from(std::io::stdin().as_fd().try_clone_to_owned()?);
    ctrlc::set_handler(|| INTERRUPTED.store(true, Ordering::Relaxed))?;
    let screen = Screen::enter()?;
    let started = Instant::now();
    let duration = Duration::from_secs(seconds);
    let mut canvas = Canvas::new(0, 0);
    let mut encoder = Encoder::new(0, 0, ui::truecolor());
    let mut previous_size = (0, 0);
    let mut previous_stage = "";
    let mut output = String::with_capacity(64 * 1024);
    let mut interrupted = false;
    let mut stopped = false;
    loop {
        let frame_started = Instant::now();
        if INTERRUPTED.load(Ordering::Relaxed) {
            interrupted = true;
            break;
        }
        size = crossterm::terminal::size().unwrap_or(size);
        output.clear();
        let changed_size = size != previous_size;
        if changed_size {
            output.push_str("\x1b[2J");
        }
        if size.0 >= 40 && size.1 >= 12 {
            let (width, height) = region(size);
            if changed_size {
                canvas = Canvas::new(width, height);
                encoder = Encoder::new(width, height, ui::truecolor());
            }
            let progress = (started.elapsed().as_secs_f32() / duration.as_secs_f32()).min(1.0);
            scene::render(
                &mut canvas,
                width,
                height,
                if down { 1.0 - progress } else { progress },
                started.elapsed().as_secs_f32(),
            );
            encoder.encode(canvas.resolve(), (1, 3), &mut output);
            if changed_size {
                text(
                    &mut output,
                    0,
                    size.0,
                    "Beam demo · no files are transferred",
                );
                let local = "Your machine";
                let sandbox = "Sandbox";
                let _ = std::fmt::Write::write_fmt(
                    &mut output,
                    format_args!(
                        "\x1b[{};{}H{local}\x1b[{};{}H{sandbox}",
                        height + 5,
                        (width as f32 * 0.2) as usize + 2 - local.len() / 2,
                        height + 5,
                        (width as f32 * 0.8) as usize + 2 - sandbox.len() / 2
                    ),
                );
                text(
                    &mut output,
                    height + 5,
                    size.0,
                    "Esc / q: exit · Ctrl-C: interrupt",
                );
            }
            let stage = scene::stage(progress, down);
            if changed_size || stage != previous_stage {
                text(&mut output, 1, size.0, stage);
                previous_stage = stage;
            }
            if progress >= 1.0 {
                present(&output)?;
                break;
            }
        } else {
            if changed_size {
                text(&mut output, 0, size.0, "Resize to 40 x 12. Esc / q: exit");
            }
            // The demo duration still runs; a tiny window must not trap the user.
            if started.elapsed() >= duration {
                present(&output)?;
                break;
            }
        }
        previous_size = size;
        present(&output)?;
        let wait = FRAME.saturating_sub(frame_started.elapsed());
        if let Some(key) = read_key(&mut keys, wait)? {
            if key == 3 {
                interrupted = true;
                break;
            }
            if matches!(key, 27 | b'q' | b'Q' | 4) {
                stopped = true;
                break;
            }
        }
    }
    drop(screen);
    if interrupted {
        ui::say("Demo interrupted. No files were transferred.");
        Err(Interrupted.into())
    } else {
        ui::say(if stopped {
            "Demo stopped. No files were transferred."
        } else {
            "Demo complete. No files were transferred."
        });
        Ok(())
    }
}

fn region(size: (u16, u16)) -> (usize, usize) {
    (
        usize::from(size.0.saturating_sub(2)).min(158),
        usize::from(size.1.saturating_sub(7)).min(40),
    )
}

fn text(output: &mut String, row: usize, width: u16, message: &str) {
    use std::fmt::Write;
    let _ = write!(output, "\x1b[{};1H\x1b[0m\x1b[2K", row + 1);
    output.extend(message.chars().take(usize::from(width.saturating_sub(1))));
}

fn present(output: &str) -> Result<()> {
    if output.is_empty() {
        return Ok(());
    }
    let mut out = std::io::stdout().lock();
    out.write_all(b"\x1b[?2026h")?;
    out.write_all(output.as_bytes())?;
    out.write_all(b"\x1b[?2026l")?;
    out.flush()?;
    Ok(())
}

#[cfg(unix)]
fn read_key(input: &mut std::fs::File, wait: Duration) -> Result<Option<u8>> {
    use std::io::Read;
    use std::os::fd::AsRawFd;
    #[repr(C)]
    struct PollFd {
        fd: std::ffi::c_int,
        events: std::ffi::c_short,
        revents: std::ffi::c_short,
    }
    #[cfg(target_os = "macos")]
    type PollCount = std::ffi::c_uint;
    #[cfg(not(target_os = "macos"))]
    type PollCount = std::ffi::c_ulong;
    unsafe extern "C" {
        fn poll(fds: *mut PollFd, count: PollCount, timeout: std::ffi::c_int) -> std::ffi::c_int;
    }
    let mut descriptor = PollFd {
        fd: input.as_raw_fd(),
        events: 1,
        revents: 0,
    };
    // SAFETY: The descriptor points to one initialized POSIX pollfd for the duration of the call.
    // PollCount follows nfds_t on the supported Linux and macOS platforms.
    let result = unsafe {
        poll(
            &mut descriptor,
            1,
            wait.as_millis().min(33) as std::ffi::c_int,
        )
    };
    if result < 0 {
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::Interrupted {
            return Ok(None);
        }
        return Err(error.into());
    }
    if result == 0 {
        return Ok(None);
    }
    if descriptor.revents & 1 != 0 {
        let mut byte = [0];
        return match input.read(&mut byte)? {
            0 => Ok(Some(4)),
            _ => Ok(Some(byte[0])),
        };
    }
    // A closed input must terminate the demo rather than spin on a hung-up descriptor.
    Ok(Some(4))
}

fn measure(down: bool) -> Result<()> {
    println!("Software rendering and encoding; 180 frames per size. No terminal writes.");
    for size in [(80, 24), (120, 40), (160, 50)] {
        let (width, height) = region(size);
        let mut canvas = Canvas::new(width, height);
        let mut encoder = Encoder::new(width, height, true);
        let mut output = String::with_capacity(width * height * 24);
        let mut times = Vec::with_capacity(180);
        let mut bytes = 0;
        for frame in 0..180 {
            output.clear();
            let start = Instant::now();
            let t = frame as f32 / 179.0;
            scene::render(
                &mut canvas,
                width,
                height,
                if down { 1.0 - t } else { t },
                frame as f32 / 30.0,
            );
            encoder.encode(canvas.resolve(), (1, 3), &mut output);
            times.push(start.elapsed().as_secs_f64() * 1000.0);
            bytes += output.len();
        }
        times.sort_by(f64::total_cmp);
        println!(
            "{}x{}: median {:.3} ms, p95 {:.3} ms, {:.0} bytes/frame, {:.0} KiB/s at 30 fps",
            size.0,
            size.1,
            times[90],
            times[171],
            bytes as f64 / 180.0,
            bytes as f64 / 180.0 * 30.0 / 1024.0
        );
    }
    std::io::stdout().flush()?;
    Ok(())
}
