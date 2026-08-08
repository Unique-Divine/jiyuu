use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use anyhow::{Context, Result, bail};

use crate::model::PaneInfo;

const PANE_FORMAT: &str = concat!(
    "#{session_id} #{q:session_name} ",
    "#{window_id} #{window_index} #{q:window_name} ",
    "#{pane_id} #{pane_index} #{q:pane_tty} ",
    "#{pane_pid} #{q:pane_current_command} ",
    "#{window_active} #{pane_active} ",
    "#{pane_width} #{pane_height} #{q:pane_title}",
);
pub const WINDOW_SUMMARY_FORMAT: &str = "#{@agent_watch_summary}";
pub const WINDOW_SUMMARY_OPTION: &str = "@agent_watch_summary";

#[derive(Clone, Debug, PartialEq, Eq)]
enum TmuxTarget {
    Default,
    SocketName(String),
    SocketPath(PathBuf),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerIdentity {
    pub socket_path: PathBuf,
    pub pid: u32,
    pub id: String,
}

#[derive(Clone, Debug)]
pub struct TmuxClient {
    target: TmuxTarget,
}

impl TmuxClient {
    pub fn new(socket_name: Option<String>) -> Self {
        let target = socket_name
            .map(TmuxTarget::SocketName)
            .unwrap_or(TmuxTarget::Default);
        Self { target }
    }

    pub fn with_socket_path(socket_path: impl Into<PathBuf>) -> Self {
        Self {
            target: TmuxTarget::SocketPath(socket_path.into()),
        }
    }

    pub fn server_identity(&self) -> Result<ServerIdentity> {
        let output = self.run_checked(&[
            "display-message",
            "-p",
            "#{q:socket_path} #{pid}",
        ])?;
        parse_server_identity(&String::from_utf8_lossy(&output.stdout))
    }

    pub fn matches_server(&self, expected: &ServerIdentity) -> bool {
        self.server_identity()
            .map(|actual| actual == *expected)
            .unwrap_or(false)
    }

    pub fn list_panes(&self) -> Result<Vec<PaneInfo>> {
        let output =
            self.run_checked(&["list-panes", "-a", "-F", PANE_FORMAT])?;
        parse_panes(&String::from_utf8_lossy(&output.stdout))
    }

    pub fn list_window_ids(&self) -> Result<Vec<String>> {
        let output =
            self.run_checked(&["list-windows", "-a", "-F", "#{window_id}"])?;
        Ok(String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect())
    }

    pub fn capture_pane(&self, pane_id: &str) -> Result<Vec<u8>> {
        let output = self.run_checked(&["capture-pane", "-p", "-t", pane_id])?;
        Ok(output.stdout)
    }

    pub fn set_window_summary(
        &self,
        window_id: &str,
        summary: &str,
    ) -> Result<()> {
        self.run_checked(&[
            "set-option",
            "-w",
            "-t",
            window_id,
            WINDOW_SUMMARY_OPTION,
            summary,
        ])?;
        Ok(())
    }

    pub fn clear_all_summaries(&self) -> Result<()> {
        for window_id in self.list_window_ids()? {
            if let Err(error) = self.set_window_summary(&window_id, "") {
                eprintln!(
                    "warning: could not clear summary for {window_id}: {error:#}"
                );
            }
        }
        Ok(())
    }

    fn command(&self) -> Command {
        let mut command = Command::new("tmux");
        match &self.target {
            TmuxTarget::Default => {}
            TmuxTarget::SocketName(socket_name) => {
                command.args(["-L", socket_name]);
            }
            TmuxTarget::SocketPath(socket_path) => {
                command.arg("-S").arg(socket_path);
            }
        }
        command
    }

    fn run_checked(&self, args: &[&str]) -> Result<Output> {
        let output =
            self.command().args(args).output().with_context(|| {
                format!("failed to run tmux {}", args.join(" "))
            })?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("tmux {} failed: {}", args.join(" "), stderr.trim());
        }
        Ok(output)
    }
}

fn parse_server_identity(output: &str) -> Result<ServerIdentity> {
    let fields = parse_fields(output.trim_end())?;
    if fields.len() != 2 {
        bail!(
            "tmux server identity expected 2 fields, got {}",
            fields.len()
        );
    }
    let socket_path = decode_tmux_escapes(&fields[0])?;
    let pid: u32 = parse_number(&fields[1], "tmux server PID")?;
    Ok(ServerIdentity {
        socket_path: Path::new(&socket_path).to_path_buf(),
        pid,
        id: format!("server-{pid}"),
    })
}

pub fn parse_panes(output: &str) -> Result<Vec<PaneInfo>> {
    output
        .lines()
        .filter(|line| !line.is_empty())
        .map(parse_pane)
        .collect()
}

fn parse_pane(line: &str) -> Result<PaneInfo> {
    let fields = parse_fields(line)?;
    if fields.len() != 15 {
        bail!(
            "expected 15 tmux pane fields, got {} in {:?}",
            fields.len(),
            line
        );
    }

    Ok(PaneInfo {
        session_id: fields[0].clone(),
        session_name: decode_tmux_escapes(&fields[1])?,
        window_id: fields[2].clone(),
        window_index: parse_number(&fields[3], "window index")?,
        window_name: decode_tmux_escapes(&fields[4])?,
        pane_id: fields[5].clone(),
        pane_index: parse_number(&fields[6], "pane index")?,
        pane_tty: decode_tmux_escapes(&fields[7])?,
        pane_pid: parse_number(&fields[8], "pane PID")?,
        current_command: decode_tmux_escapes(&fields[9])?,
        window_active: parse_bool(&fields[10], "window_active")?,
        pane_active: parse_bool(&fields[11], "pane_active")?,
        width: parse_number(&fields[12], "pane width")?,
        height: parse_number(&fields[13], "pane height")?,
        title: decode_tmux_escapes(&fields[14])?,
    })
}

fn parse_fields(line: &str) -> Result<Vec<String>> {
    shell_words::split(line)
        .with_context(|| format!("invalid tmux quoted fields in {line:?}"))
}

fn decode_tmux_escapes(value: &str) -> Result<String> {
    let mut decoded = String::with_capacity(value.len());
    let mut characters = value.chars().peekable();
    while let Some(character) = characters.next() {
        if character != '\\' {
            decoded.push(character);
            continue;
        }

        let Some(escaped) = characters.next() else {
            bail!("tmux quoted field ended with an incomplete escape");
        };
        match escaped {
            '\\' => decoded.push('\\'),
            'n' => decoded.push('\n'),
            'r' => decoded.push('\r'),
            't' => decoded.push('\t'),
            '0'..='7' => {
                let mut octal = String::from(escaped);
                for _ in 0..2 {
                    if matches!(characters.peek(), Some('0'..='7')) {
                        octal.push(
                            characters
                                .next()
                                .expect("peeked octal digit should exist"),
                        );
                    }
                }
                let byte = u8::from_str_radix(&octal, 8).with_context(|| {
                    format!("invalid tmux octal escape \\{octal}")
                })?;
                decoded.push(char::from(byte));
            }
            other => {
                decoded.push('\\');
                decoded.push(other);
            }
        }
    }
    Ok(decoded)
}

fn parse_number<T>(value: &str, field: &str) -> Result<T>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    value
        .parse()
        .map_err(|error| anyhow::anyhow!("invalid {field} {value:?}: {error}"))
}

fn parse_bool(value: &str, field: &str) -> Result<bool> {
    match value {
        "0" => Ok(false),
        "1" => Ok(true),
        _ => bail!("invalid {field} boolean {value:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tmux_pane_record() {
        let record = [
            "$0",
            r"dev\ session",
            "@2",
            "3",
            r"work\\tqueue",
            "%12",
            "1",
            "/dev/pts/4",
            "1234",
            "agent",
            "1",
            "1",
            "90",
            "40",
            r"Agent\ title\\tready",
        ]
        .join(" ");

        let panes = parse_panes(&record).expect("pane should parse");
        assert_eq!(panes.len(), 1);
        assert_eq!(panes[0].pane_id, "%12");
        assert_eq!(panes[0].window_id, "@2");
        assert_eq!(panes[0].session_name, "dev session");
        assert_eq!(panes[0].window_name, "work\tqueue");
        assert_eq!(panes[0].title, "Agent title\tready");
        assert!(panes[0].selected());
    }

    #[test]
    fn rejects_incomplete_tmux_record() {
        let error = parse_panes("$0 dev").expect_err("record should fail");
        assert!(error.to_string().contains("expected 15"));
    }

    #[test]
    fn parses_server_socket_and_pid_identity() {
        let identity =
            parse_server_identity("/tmp/tmux-1000/socket\\ name 3047\n")
                .expect("identity should parse");
        assert_eq!(
            identity.socket_path,
            PathBuf::from("/tmp/tmux-1000/socket name")
        );
        assert_eq!(identity.pid, 3047);
        assert_eq!(identity.id, "server-3047");
    }

    #[test]
    fn decodes_tmux_control_and_literal_backslash_escapes() {
        assert_eq!(
            decode_tmux_escapes(r"tab\tnewline\nslash\\literal\\t")
                .expect("escapes should decode"),
            "tab\tnewline\nslash\\literal\\t"
        );
        assert_eq!(
            decode_tmux_escapes(r"escape\033")
                .expect("octal escape should decode"),
            "escape\u{1b}"
        );
    }
}
