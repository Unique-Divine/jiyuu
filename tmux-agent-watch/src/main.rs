use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use tmux_agent_watch::lifecycle::WatcherLock;
use tmux_agent_watch::model::{
    DEFAULT_CHANGING_SYMBOL, DEFAULT_STATIC_SYMBOL, DEFAULT_TMUX_LABEL,
    DEFAULT_TMUX_SEPARATOR, WindowSummary,
};
use tmux_agent_watch::render::{
    render_inspect, render_list, render_status, validate_snapshot_freshness,
};
use tmux_agent_watch::snapshot::{read_snapshot, write_snapshot};
use tmux_agent_watch::tmux::{ServerIdentity, TmuxClient};
use tmux_agent_watch::tracker::Tracker;

const DEFAULT_STALE_AFTER_MS: u64 = 10_000;

#[derive(Debug)]
struct WatchConfig {
    interval: Duration,
    static_after: Duration,
    once: bool,
    publish_tmux: bool,
    tmux_label: String,
    tmux_separator: String,
    changing_symbol: String,
    static_symbol: String,
}

#[derive(Debug, Parser)]
#[command(
    name = "tmux-agent-watch",
    about = "Observe visible activity in Cursor and Codex tmux panes"
)]
struct Cli {
    /// Target a named tmux socket (`tmux -L <name>`).
    #[arg(long, global = true)]
    socket: Option<String>,

    /// Target an exact tmux socket path (`tmux -S <path>`).
    #[arg(long, global = true, conflicts_with = "socket")]
    socket_path: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Continuously observe agent panes and publish state.
    Watch {
        #[arg(long, default_value_t = 500)]
        interval_ms: u64,

        #[arg(long, default_value_t = 2_500)]
        static_after_ms: u64,

        /// Collect one snapshot and exit.
        #[arg(long)]
        once: bool,

        /// Do not publish per-window tmux user options.
        #[arg(long)]
        no_tmux_status: bool,

        /// Label shown before per-window agent counts.
        #[arg(long, default_value_t = DEFAULT_TMUX_LABEL.to_owned())]
        tmux_label: String,

        /// Text separating the tmux window name from agent counts.
        #[arg(long, default_value_t = DEFAULT_TMUX_SEPARATOR.to_owned())]
        tmux_separator: String,

        /// Symbol shown after the changing-pane count.
        #[arg(long, default_value_t = DEFAULT_CHANGING_SYMBOL.to_owned())]
        changing_symbol: String,

        /// Symbol shown after the static-pane count.
        #[arg(long, default_value_t = DEFAULT_STATIC_SYMBOL.to_owned())]
        static_symbol: String,
    },

    /// Print the latest pane table.
    List {
        #[arg(long)]
        allow_stale: bool,
    },

    /// Print a compact summary for scripts and status lines.
    Status {
        #[arg(long)]
        window_id: Option<String>,

        #[arg(long)]
        allow_stale: bool,
    },

    /// Explain the latest evidence for one agent pane.
    Inspect {
        pane_id: String,

        #[arg(long)]
        allow_stale: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let client = match cli.socket_path {
        Some(socket_path) => TmuxClient::with_socket_path(socket_path),
        None => TmuxClient::new(cli.socket),
    };
    match cli.command {
        Command::Watch {
            interval_ms,
            static_after_ms,
            once,
            no_tmux_status,
            tmux_label,
            tmux_separator,
            changing_symbol,
            static_symbol,
        } => run_watch(
            client,
            WatchConfig {
                interval: Duration::from_millis(interval_ms),
                static_after: Duration::from_millis(static_after_ms),
                once,
                publish_tmux: !no_tmux_status,
                tmux_label,
                tmux_separator,
                changing_symbol,
                static_symbol,
            },
        ),
        Command::List { allow_stale } => {
            let snapshot = load_snapshot(&client, allow_stale)?;
            println!("{}", render_list(&snapshot));
            Ok(())
        }
        Command::Status {
            window_id,
            allow_stale,
        } => {
            let snapshot = load_snapshot(&client, allow_stale)?;
            println!("{}", render_status(&snapshot, window_id.as_deref()));
            Ok(())
        }
        Command::Inspect {
            pane_id,
            allow_stale,
        } => {
            let snapshot = load_snapshot(&client, allow_stale)?;
            println!("{}", render_inspect(&snapshot, &pane_id)?);
            Ok(())
        }
    }
}

fn run_watch(client: TmuxClient, config: WatchConfig) -> Result<()> {
    if config.interval.is_zero() {
        anyhow::bail!("--interval-ms must be greater than zero");
    }
    if config.static_after.is_zero() {
        anyhow::bail!("--static-after-ms must be greater than zero");
    }

    let identity = client.server_identity()?;
    let Some(watcher_lock) = WatcherLock::try_acquire(&identity)? else {
        eprintln!("watcher already running for tmux server {}", identity.pid);
        return Ok(());
    };

    let running = Arc::new(AtomicBool::new(true));
    let signal_running = Arc::clone(&running);
    ctrlc::set_handler(move || signal_running.store(false, Ordering::SeqCst))
        .context("failed to install shutdown handler")?;

    if config.publish_tmux {
        client.clear_all_summaries()?;
    }
    let result = watch_loop(&client, &identity, &config, &running);
    if config.publish_tmux
        && client.matches_server(&identity)
        && let Err(error) = client.clear_all_summaries()
    {
        eprintln!("warning: failed to clear tmux status summaries: {error:#}");
    }
    drop(watcher_lock);
    result
}

fn watch_loop(
    client: &TmuxClient,
    identity: &ServerIdentity,
    config: &WatchConfig,
    running: &AtomicBool,
) -> Result<()> {
    let mut tracker = Tracker::new(config.static_after);
    let mut published = BTreeMap::<String, WindowSummary>::new();
    let mut announced_path = false;

    while running.load(Ordering::SeqCst) {
        if !client.matches_server(identity) {
            eprintln!("tmux server exited; watcher stopping");
            break;
        }
        let cycle_started = Instant::now();
        let panes = match client.list_panes() {
            Ok(panes) => panes,
            Err(_error) if !client.matches_server(identity) => {
                eprintln!("tmux server exited; watcher stopping");
                break;
            }
            Err(error) => return Err(error),
        };
        let mut observations = Vec::new();
        for pane in panes.into_iter().filter(|pane| pane.agent_kind().is_some())
        {
            match client.capture_pane(&pane.pane_id) {
                Ok(capture) => observations.push((pane, capture)),
                Err(error) => {
                    eprintln!(
                        "warning: could not capture {}: {error:#}",
                        pane.pane_id
                    );
                }
            }
        }

        let snapshot = tracker.observe(
            cycle_started,
            unix_time_ms()?,
            identity.id.clone(),
            observations,
        );
        let path = write_snapshot(&snapshot)?;
        if !announced_path {
            eprintln!("watching tmux; snapshot: {}", path.display());
            announced_path = true;
        }

        if config.publish_tmux {
            let summaries = snapshot.window_summaries();
            if summaries != published {
                client.clear_all_summaries()?;
                for (window_id, summary) in &summaries {
                    if let Err(error) = client.set_window_summary(
                        window_id,
                        &summary.render_tmux_segment(
                            &config.tmux_label,
                            &config.tmux_separator,
                            &config.changing_symbol,
                            &config.static_symbol,
                        ),
                    ) {
                        eprintln!(
                            "warning: could not publish summary for {window_id}: {error:#}"
                        );
                    }
                }
                published = summaries;
            }
        }

        if config.once {
            break;
        }
        if let Some(remaining) =
            config.interval.checked_sub(cycle_started.elapsed())
        {
            thread::sleep(remaining);
        }
    }
    Ok(())
}

fn load_snapshot(
    client: &TmuxClient,
    allow_stale: bool,
) -> Result<tmux_agent_watch::model::Snapshot> {
    let identity = client.server_identity()?;
    let snapshot = read_snapshot(&identity.id)?;
    if !allow_stale {
        validate_snapshot_freshness(
            &snapshot,
            unix_time_ms()?,
            DEFAULT_STALE_AFTER_MS,
        )?;
    }
    Ok(snapshot)
}

fn unix_time_ms() -> Result<u64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system time is before the Unix epoch")?
        .as_millis() as u64)
}
