//! The main run loop: draw, then wait for input or a periodic tick. Liveness
//! results are drained each tick without blocking the draw.

use std::time::{Duration, Instant};

use anyhow::Result;
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event, KeyEventKind};

use crate::app::App;
use crate::{ui, update};

const TICK: Duration = Duration::from_millis(200);

pub fn run(mut terminal: DefaultTerminal, mut app: App) -> Result<()> {
    let result = event_loop(&mut terminal, &mut app);
    // Best-effort: never leave a copied vault secret in the clipboard on exit —
    // including an error-driven exit that bypasses the normal quit handler.
    if app.clipboard_clear_at.is_some() {
        update::force_clear_clipboard(&mut app);
    }
    result
}

fn event_loop(terminal: &mut DefaultTerminal, app: &mut App) -> Result<()> {
    let mut last_tick = Instant::now();

    while !app.should_quit {
        terminal.draw(|f| ui::draw(f, app))?;

        let timeout = TICK.saturating_sub(last_tick.elapsed());
        if event::poll(timeout)? {
            // On Windows the console emits both key-down and key-up; only act on
            // Press to avoid double input.
            if let Event::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press
            {
                update::handle_key(app, key, terminal)?;
            }
        }

        if last_tick.elapsed() >= TICK {
            app.on_tick();
            if app.drain_liveness() {
                // drain_liveness returns true only when a reachability rank changed,
                // so the Status sort re-orders just when its key actually moved.
                app.resort_after_liveness();
            }
            // Apply any completed SFTP browse ops so the panes refresh without
            // blocking the draw (no-op when not browsing).
            app.drain_sftp_browser();
            // Pick up a finished ssh-agent probe (no-op when none is in flight),
            // so the key manager's badges settle without blocking the draw.
            app.drain_agent();
            update::tick_clipboard(app);
            last_tick = Instant::now();
        }
    }
    Ok(())
}
