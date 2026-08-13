use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub const DEFAULT_CHANGING_SYMBOL: &str = "↻";
pub const DEFAULT_STATIC_SYMBOL: &str = ".";
pub const DEFAULT_TMUX_LABEL: &str = "";
pub const DEFAULT_TMUX_SEPARATOR: &str = " │ ";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentKind {
    Cursor,
    Codex,
}

impl AgentKind {
    pub fn from_command(command: &str) -> Option<Self> {
        match command {
            "agent" => Some(Self::Cursor),
            "codex" => Some(Self::Codex),
            _ => None,
        }
    }
}

impl std::fmt::Display for AgentKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cursor => write!(formatter, "cursor"),
            Self::Codex => write!(formatter, "codex"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Activity {
    Changing,
    Static,
}

impl std::fmt::Display for Activity {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Changing => write!(formatter, "changing"),
            Self::Static => write!(formatter, "static"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneInfo {
    pub session_id: String,
    pub session_name: String,
    pub window_id: String,
    pub window_index: u32,
    pub window_name: String,
    pub pane_id: String,
    pub pane_index: u32,
    pub pane_tty: String,
    pub pane_pid: u32,
    pub current_command: String,
    pub window_active: bool,
    pub pane_active: bool,
    pub width: u16,
    pub height: u16,
    pub title: String,
}

impl PaneInfo {
    pub fn agent_kind(&self) -> Option<AgentKind> {
        AgentKind::from_command(&self.current_command)
    }

    pub fn selected(&self) -> bool {
        self.window_active && self.pane_active
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneSnapshot {
    pub pane_id: String,
    pub session_id: String,
    pub session_name: String,
    pub window_id: String,
    pub window_index: u32,
    pub window_name: String,
    pub pane_index: u32,
    pub agent: AgentKind,
    pub title: String,
    pub activity: Activity,
    pub quiet_ms: u64,
    pub selected: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowSummary {
    pub changing: usize,
    pub static_count: usize,
}

impl WindowSummary {
    pub fn render_compact(&self) -> String {
        self.render_compact_with_symbols(
            DEFAULT_CHANGING_SYMBOL,
            DEFAULT_STATIC_SYMBOL,
        )
    }

    pub fn render_compact_with_symbols(
        &self,
        changing_symbol: &str,
        static_symbol: &str,
    ) -> String {
        let mut parts = Vec::new();
        if self.changing > 0 {
            parts.push(changing_symbol.repeat(self.changing));
        }
        if self.static_count > 0 {
            parts.push(static_symbol.repeat(self.static_count));
        }
        parts.join(" ")
    }

    pub fn render_tmux_segment(
        &self,
        label: &str,
        separator: &str,
        changing_symbol: &str,
        static_symbol: &str,
    ) -> String {
        let counts =
            self.render_compact_with_symbols(changing_symbol, static_symbol);
        if counts.is_empty() {
            return String::new();
        }
        if label.is_empty() {
            format!("{separator}{counts}")
        } else {
            format!("{separator}{label} {counts}")
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub schema_version: u32,
    pub observed_at_ms: u64,
    pub tmux_server: String,
    pub panes: Vec<PaneSnapshot>,
}

impl Snapshot {
    pub fn window_summaries(&self) -> BTreeMap<String, WindowSummary> {
        let mut summaries = BTreeMap::new();
        for pane in &self.panes {
            let summary = summaries
                .entry(pane.window_id.clone())
                .or_insert_with(WindowSummary::default);
            match pane.activity {
                Activity::Changing => summary.changing += 1,
                Activity::Static => summary.static_count += 1,
            }
        }
        summaries
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_kind_only_recognizes_supported_commands() {
        assert_eq!(AgentKind::from_command("agent"), Some(AgentKind::Cursor));
        assert_eq!(AgentKind::from_command("codex"), Some(AgentKind::Codex));
        assert_eq!(AgentKind::from_command("nvim"), None);
    }

    #[test]
    fn compact_summary_omits_zero_counts() {
        let summary = WindowSummary {
            changing: 2,
            static_count: 2,
        };
        let expected = format!(
            "{0}{0} {1}{1}",
            DEFAULT_CHANGING_SYMBOL, DEFAULT_STATIC_SYMBOL
        );
        assert_eq!(summary.render_compact(), expected);
        assert_eq!(
            summary.render_tmux_segment(
                "AI",
                DEFAULT_TMUX_SEPARATOR,
                DEFAULT_CHANGING_SYMBOL,
                DEFAULT_STATIC_SYMBOL,
            ),
            format!(" │ AI {expected}")
        );
        assert_eq!(WindowSummary::default().render_compact(), "");
    }
}
