//! sshm — a terminal UI for browsing, editing, and connecting to SSH hosts
//! defined in `~/.ssh/config`.

mod app;
mod config;
mod error;
mod event_loop;
mod os;
mod secure_fs;
mod ui;
mod update;

use std::path::PathBuf;

use anyhow::Result;

use crate::app::App;
use crate::config::{SshConfig, default_config_path};

const HELP: &str = "\
sshm — TUI SSH host manager (~/.ssh/config)

USAGE:
    sshm [OPTIONS]

OPTIONS:
    (no args)         launch the interactive TUI
    -c, --config PATH use an alternate config file (default: ~/.ssh/config)
    -l, --list        print configured hosts and exit (non-interactive)
    -V, --version     print version and exit
    -h, --help        print this help and exit
";

enum Command {
    Tui,
    List,
    Version,
    Help,
}

fn main() -> Result<()> {
    // SSH_ASKPASS helper mode is selected by the PRESENCE of SSHM_ASKPASS_CHANNEL
    // in the environment — OpenSSH execs us as `sshm "<prompt>"` with no flag. This
    // runs BEFORE any arg parsing, so a prompt beginning with `-` is never
    // misparsed as a flag and never reaches the `other => exit(2)` arm, and before
    // ratatui/config init so the helper stays a thin, side-effect-free relay.
    if let Some(mut bytes) =
        crate::os::askpass::run_helper(std::env::args().nth(1), |k| std::env::var(k).ok())
    {
        use std::io::Write;
        use zeroize::Zeroize;
        if bytes.is_empty() {
            // No match / channel error: exit non-zero with no stdout.
            std::process::exit(1);
        }
        let mut out = std::io::stdout();
        let _ = out.write_all(&bytes);
        let _ = out.flush();
        // `process::exit` runs no destructors, so the `Zeroizing` Drop would be
        // skipped — scrub the printed plaintext explicitly before exiting.
        bytes.zeroize();
        std::process::exit(0);
    }

    let mut command = Command::Tui;
    let mut config_path: Option<PathBuf> = None;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--config" | "-c" => {
                config_path = match args.next() {
                    Some(p) => Some(PathBuf::from(p)),
                    None => {
                        eprintln!("--config requires a path argument");
                        std::process::exit(2);
                    }
                };
            }
            "--list" | "-l" => command = Command::List,
            "--version" | "-V" => command = Command::Version,
            "--help" | "-h" => command = Command::Help,
            other => {
                eprintln!("unknown argument: {other}\n");
                print!("{HELP}");
                std::process::exit(2);
            }
        }
    }

    match command {
        Command::Version => {
            println!("sshm {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Command::Help => {
            print!("{HELP}");
            Ok(())
        }
        Command::List => cmd_list(resolve_path(config_path)?),
        Command::Tui => run_tui(resolve_path(config_path)?),
    }
}

fn resolve_path(custom: Option<PathBuf>) -> Result<PathBuf> {
    match custom {
        Some(p) => Ok(p),
        None => Ok(default_config_path()?),
    }
}

/// Non-interactive: print the hosts parsed from the config file.
fn cmd_list(path: PathBuf) -> Result<()> {
    let cfg = SshConfig::load(path.clone())?;
    let views = cfg.host_views();
    if views.is_empty() {
        println!("(no Host entries in {})", path.display());
        return Ok(());
    }
    println!("{} host(s) in {}:", views.len(), path.display());
    for (_, h) in &views {
        let host = h.host_name.as_deref().unwrap_or("-");
        let user = h.user.as_deref().unwrap_or("-");
        let port = h.port.as_deref().unwrap_or("22");
        println!("  {:<24} {}@{}:{}", h.alias(), user, host, port);
    }
    Ok(())
}

/// Launch the interactive terminal UI.
fn run_tui(path: PathBuf) -> Result<()> {
    let terminal = ratatui::init();
    let result = App::new(path).and_then(|app| event_loop::run(terminal, app));
    ratatui::restore();

    if let Err(err) = &result {
        eprintln!("error: {err:?}");
    }
    result
}
