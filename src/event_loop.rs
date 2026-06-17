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
    let mut last_tick = Instant::now();

    while !app.should_quit {
        terminal.draw(|f| ui::draw(f, &mut app))?;

        let timeout = TICK.saturating_sub(last_tick.elapsed());
        if event::poll(timeout)? {
            // On Windows the console emits both key-down and key-up; only act on
            // Press to avoid double input.
            if let Event::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press
            {
                update::handle_key(&mut app, key, &mut terminal)?;
            }
        }

        if last_tick.elapsed() >= TICK {
            app.on_tick();
            app.drain_liveness();
            update::tick_clipboard(&mut app);
            last_tick = Instant::now();
        }
    }
    Ok(())
}
