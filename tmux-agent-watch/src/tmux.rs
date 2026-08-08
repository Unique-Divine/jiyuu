use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use anyhow::{Context, Result, bail};

use crate::model::PaneInfo;

const FIELD_SEPARATOR: char = '\t';
const PANE_FORMAT: &str = concat!(
    "#{session_id}\t#{session_name}\t",
    "#{window_id}\t#{window_index}\t#{window_name}\t",
    "#{pane_id}\t#{pane_index}\t#{pane_tty}\t",
    "#{pane_pid}\t#{pane_current_command}\t",
    "#{window_active}\t#{pane_active}\t",
    "#{pane_width}\t#{pane_height}\t#{pane_title}",
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
            "#{socket_path}\t#{pid}",
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
    let line = output.trim_end();
    let (socket_path, pid) = line
        .split_once(FIELD_SEPARATOR)
        .context("tmux server identity did not contain socket path and PID")?;
    let pid: u32 = parse_number(pid, "tmux server PID")?;
    Ok(ServerIdentity {
        socket_path: Path::new(socket_path).to_path_buf(),
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
    let fields: Vec<&str> = line.split(FIELD_SEPARATOR).collect();
    if fields.len() != 15 {
        bail!(
            "expected 15 tmux pane fields, got {} in {:?}",
            fields.len(),
            line
        );
    }

    Ok(PaneInfo {
        session_id: fields[0].to_owned(),
        session_name: fields[1].to_owned(),
        window_id: fields[2].to_owned(),
        window_index: parse_number(fields[3], "window index")?,
        window_name: fields[4].to_owned(),
        pane_id: fields[5].to_owned(),
        pane_index: parse_number(fields[6], "pane index")?,
        pane_tty: fields[7].to_owned(),
        pane_pid: parse_number(fields[8], "pane PID")?,
        current_command: fields[9].to_owned(),
        window_active: parse_bool(fields[10], "window_active")?,
        pane_active: parse_bool(fields[11], "pane_active")?,
        width: parse_number(fields[12], "pane width")?,
        height: parse_number(fields[13], "pane height")?,
        title: fields[14].to_owned(),
    })
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
        let separator = FIELD_SEPARATOR;
        let record = [
            "$0",
            "dev",
            "@2",
            "3",
            "work",
            "%12",
            "1",
            "/dev/pts/4",
            "1234",
            "agent",
            "1",
            "1",
            "90",
            "40",
            "Agent title",
        ]
        .join(&separator.to_string());

        let panes = parse_panes(&record).expect("pane should parse");
        assert_eq!(panes.len(), 1);
        assert_eq!(panes[0].pane_id, "%12");
        assert_eq!(panes[0].window_id, "@2");
        assert_eq!(panes[0].title, "Agent title");
        assert!(panes[0].selected());
    }

    #[test]
    fn rejects_incomplete_tmux_record() {
        let error = parse_panes("$0\tdev").expect_err("record should fail");
        assert!(error.to_string().contains("expected 15"));
    }

    #[test]
    fn parses_server_socket_and_pid_identity() {
        let identity = parse_server_identity("/tmp/tmux-1000/default\t3047\n")
            .expect("identity should parse");
        assert_eq!(
            identity.socket_path,
            PathBuf::from("/tmp/tmux-1000/default")
        );
        assert_eq!(identity.pid, 3047);
        assert_eq!(identity.id, "server-3047");
    }
}
