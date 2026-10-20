// ABOUTME: Preview Beam's progress effects without allocating a sandbox or transferring files.
// ABOUTME: Use BEAM_EFFECT=off|graphics|shader, pass down to reverse the direction, and show for the transfer animation.
#![allow(dead_code)]
#[path = "../src/raster.rs"]
mod raster;
#[path = "../src/scene.rs"]
mod scene;
#[path = "../src/show.rs"]
mod show;
#[path = "../src/transporter.rs"]
mod transporter;
#[path = "../src/ui.rs"]
mod ui;

fn main() -> anyhow::Result<()> {
    let home = std::env::args().any(|arg| arg == "down");
    let message = std::env::args().any(|arg| arg == "message");
    let animated = std::env::args().any(|arg| arg == "show");
    transporter::direction(home);
    let _busy = ui::Busy::start();
    ui::say("Beam display preview — no files are transferred.");
    ui::step("destination", if home { "home" } else { "preview sandbox" });
    ui::task("check", "checking preview", || {
        std::thread::sleep(std::time::Duration::from_millis(150));
        Ok(())
    })?;
    let show = if animated {
        show::Show::start(home)
    } else {
        None
    };
    // The same legs as a transfer: the archive moves, then the workspace forms at the destination.
    if let Some(show) = &show {
        show.toward(0.6);
    }
    ui::task(
        if home { "download" } else { "upload" },
        "previewing transfer effect",
        || {
            std::thread::sleep(std::time::Duration::from_secs(2));
            if message {
                ui::say("A mid-transfer message remains visible.");
            }
            std::thread::sleep(std::time::Duration::from_secs(3));
            Ok(())
        },
    )?;
    if let Some(show) = &show {
        show.toward(1.0);
    }
    ui::task(
        if home { "review" } else { "restore" },
        "previewing arrival",
        || {
            std::thread::sleep(std::time::Duration::from_secs(2));
            Ok(())
        },
    )?;
    if let Some(show) = &show {
        show.arrive();
    }
    ui::task("launch", "previewing the work after arrival", || {
        std::thread::sleep(std::time::Duration::from_secs(2));
        Ok(())
    })?;
    drop(show);
    ui::success("Preview complete. Previous output stays in scrollback.");
    Ok(())
}
