use anyhow::{Result, bail};

use crate::model::{Activity, PaneSnapshot, Snapshot, WindowSummary};

pub fn render_list(snapshot: &Snapshot) -> String {
    let mut lines = vec![format!(
        "{:<6} {:<16} {:<24} {:<7} {:<9} {:>8}  {}",
        "PANE", "WINDOW", "TITLE", "AGENT", "SCREEN", "QUIET", "LOCATION"
    )];
    for pane in &snapshot.panes {
        lines.push(format!(
            "{:<6} {:<16} {:<24} {:<7} {:<9} {:>7.1}s  {}",
            pane.pane_id,
            truncate(&pane.window_name, 16),
            truncate(&pane.title, 24),
            pane.agent,
            pane.activity,
            pane.quiet_ms as f64 / 1_000.0,
            if pane.selected { "here" } else { "elsewhere" },
        ));
    }
    lines.join("\n")
}

pub fn render_status(snapshot: &Snapshot, window_id: Option<&str>) -> String {
    if let Some(window_id) = window_id {
        return snapshot
            .window_summaries()
            .get(window_id)
            .map(WindowSummary::render_compact)
            .unwrap_or_default();
    }

    let mut combined = WindowSummary::default();
    for pane in &snapshot.panes {
        match pane.activity {
            Activity::Changing => combined.changing += 1,
            Activity::Static => combined.static_count += 1,
        }
    }
    combined.render_compact()
}

pub fn render_inspect(snapshot: &Snapshot, pane_id: &str) -> Result<String> {
    let pane = snapshot
        .panes
        .iter()
        .find(|pane| pane.pane_id == pane_id)
        .ok_or_else(|| {
            anyhow::anyhow!("pane {pane_id} is not in the latest agent snapshot")
        })?;
    Ok(render_pane_details(pane))
}

fn render_pane_details(pane: &PaneSnapshot) -> String {
    [
        format!("pane: {}", pane.pane_id),
        format!("session: {} ({})", pane.session_name, pane.session_id),
        format!(
            "window: {}:{} ({})",
            pane.window_index, pane.window_name, pane.window_id
        ),
        format!("pane_index: {}", pane.pane_index),
        format!("title: {}", pane.title),
        format!("agent: {}", pane.agent),
        format!("screen: {}", pane.activity),
        format!("quiet_ms: {}", pane.quiet_ms),
        format!("selected: {}", pane.selected),
        "meaning: screen activity only; not agent readiness".to_owned(),
    ]
    .join("\n")
}

fn truncate(value: &str, maximum: usize) -> String {
    let count = value.chars().count();
    if count <= maximum {
        return value.to_owned();
    }
    if maximum < 2 {
        return "…".to_owned();
    }
    let prefix: String = value.chars().take(maximum - 1).collect();
    format!("{prefix}…")
}

pub fn validate_snapshot_freshness(
    snapshot: &Snapshot,
    now_ms: u64,
    stale_after_ms: u64,
) -> Result<()> {
    if now_ms.saturating_sub(snapshot.observed_at_ms) > stale_after_ms {
        bail!(
            "snapshot is stale (observed {} ms ago); start `tmux-agent-watch watch`",
            now_ms.saturating_sub(snapshot.observed_at_ms)
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        AgentKind, DEFAULT_CHANGING_SYMBOL, DEFAULT_STATIC_SYMBOL, PaneSnapshot,
    };

    fn snapshot() -> Snapshot {
        Snapshot {
            schema_version: 1,
            observed_at_ms: 1_000,
            tmux_server: "default".to_owned(),
            panes: vec![
                PaneSnapshot {
                    pane_id: "%1".to_owned(),
                    session_id: "$0".to_owned(),
                    session_name: "dev".to_owned(),
                    window_id: "@0".to_owned(),
                    window_index: 0,
                    window_name: "work".to_owned(),
                    pane_index: 0,
                    agent: AgentKind::Cursor,
                    title: "First agent".to_owned(),
                    activity: Activity::Changing,
                    quiet_ms: 100,
                    selected: true,
                },
                PaneSnapshot {
                    pane_id: "%2".to_owned(),
                    session_id: "$0".to_owned(),
                    session_name: "dev".to_owned(),
                    window_id: "@0".to_owned(),
                    window_index: 0,
                    window_name: "work".to_owned(),
                    pane_index: 1,
                    agent: AgentKind::Codex,
                    title: "Second agent".to_owned(),
                    activity: Activity::Static,
                    quiet_ms: 5_000,
                    selected: false,
                },
            ],
        }
    }

    #[test]
    fn renders_global_and_window_status() {
        let expected =
            format!("{DEFAULT_CHANGING_SYMBOL} {DEFAULT_STATIC_SYMBOL}");
        assert_eq!(render_status(&snapshot(), None), expected);
        assert_eq!(render_status(&snapshot(), Some("@0")), expected);
        assert_eq!(render_status(&snapshot(), Some("@missing")), "");
    }

    #[test]
    fn inspect_explains_observational_semantics() {
        let rendered =
            render_inspect(&snapshot(), "%2").expect("pane should exist");
        assert!(rendered.contains("screen: static"));
        assert!(rendered.contains("not agent readiness"));
    }

    #[test]
    fn detects_stale_snapshots() {
        assert!(validate_snapshot_freshness(&snapshot(), 2_000, 2_000).is_ok());
        assert!(validate_snapshot_freshness(&snapshot(), 4_000, 2_000).is_err());
    }
}
