//! herdr-aa-notes — a persistent markdown notes pane for herdr: one scrollable
//! note with preview/edit modes, autosaved to a JSON file that survives
//! restarts.
//!
//! The `--*` stdin→stdout helper modes serve the launcher scripts — see
//! launch.rs.

mod app;
mod ipc;
mod launch;
mod markdown;
mod state;

use std::io::Read;
use std::time::Duration;

use crossterm::event::{self, Event};

fn main() -> std::io::Result<()> {
    match std::env::args().nth(1).as_deref() {
        Some("--launch-decision") => {
            println!("{}", launch::launch_decision(&read_stdin()?, state::unix_now()));
            return Ok(());
        }
        Some("--focused-pane") => {
            println!("{}", launch::focused_pane(&read_stdin()?));
            return Ok(());
        }
        Some("--open-plan") => {
            println!("{}", launch::open_plan(&read_stdin()?));
            return Ok(());
        }
        Some(other) => {
            eprintln!("herdr-aa-notes: unknown argument `{other}`");
            eprintln!("usage: herdr-aa-notes [--launch-decision|--focused-pane|--open-plan]");
            std::process::exit(2);
        }
        None => {}
    }

    let mut terminal = ratatui::init();
    let mut app = app::App::new();
    let result = run(&mut terminal, &mut app);
    app.finalize();
    ratatui::restore();
    result
}

/// Event loop with a short poll so the liveness heartbeat keeps stamping and
/// the debounced autosave keeps flushing while idle.
fn run(terminal: &mut ratatui::DefaultTerminal, app: &mut app::App) -> std::io::Result<()> {
    loop {
        terminal.draw(|frame| app.draw(frame))?;
        // Non-key events (resize, focus, …) simply fall through to a redraw.
        if event::poll(Duration::from_millis(500))?
            && let Event::Key(key) = event::read()?
            && app.on_key(key)
        {
            return Ok(());
        }
        // Every iteration — not only on poll timeout — so sustained input
        // (held-key auto-repeat, a long paste) can never starve the liveness
        // stamp into REPLACE territory or hold back the debounced autosave.
        // Both self-throttle, so this is cheap.
        app.heartbeat();
        app.maybe_flush();
    }
}

fn read_stdin() -> std::io::Result<String> {
    let mut buf = String::new();
    std::io::stdin().read_to_string(&mut buf)?;
    Ok(buf)
}
