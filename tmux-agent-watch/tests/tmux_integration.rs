use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tmux_agent_watch::model::{
    DEFAULT_CHANGING_SYMBOL, DEFAULT_STATIC_SYMBOL, DEFAULT_TMUX_LABEL,
    DEFAULT_TMUX_SEPARATOR, WindowSummary,
};
use tmux_agent_watch::tmux::{TmuxClient, WINDOW_SUMMARY_FORMAT};

struct TestServer {
    socket: String,
    active: bool,
}

impl TestServer {
    fn start() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be valid")
            .as_nanos();
        let socket = format!("agent-watch-test-{}-{nonce}", std::process::id());
        run_tmux(&socket, &["new-session", "-d", "-s", "test", "sleep 30"]);
        Self {
            socket,
            active: true,
        }
    }

    fn run(&self, args: &[&str]) -> String {
        run_tmux(&self.socket, args)
    }

    fn kill(&mut self) {
        if self.active {
            run_tmux(&self.socket, &["kill-server"]);
            self.active = false;
        }
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        if self.active {
            let _ = Command::new("tmux")
                .args(["-L", &self.socket, "kill-server"])
                .output();
        }
    }
}

fn spawn_watcher(socket: &str) -> Child {
    Command::new(env!("CARGO_BIN_EXE_tmux-agent-watch"))
        .args([
            "--socket",
            socket,
            "watch",
            "--no-tmux-status",
            "--interval-ms",
            "50",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("watcher should start")
}

fn wait_for_exit(child: &mut Child) {
    for _ in 0..40 {
        if child
            .try_wait()
            .expect("watcher status should be readable")
            .is_some()
        {
            return;
        }
        thread::sleep(Duration::from_millis(50));
    }
    child.kill().expect("hung watcher should be killed");
    panic!("watcher did not exit after tmux server stopped");
}

fn run_tmux(socket: &str, args: &[&str]) -> String {
    let output = Command::new("tmux")
        .env("SHELL", "/bin/sh")
        .args(["-L", socket, "-f", "/dev/null"])
        .args(args)
        .output()
        .expect("tmux should execute");
    assert!(
        output.status.success(),
        "tmux {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("tmux output should be UTF-8")
        .trim_end()
        .to_owned()
}

#[test]
fn observes_isolated_tmux_lifecycle_and_display_options() {
    let server = TestServer::start();
    let client = TmuxClient::new(Some(server.socket.clone()));
    let identity = client
        .server_identity()
        .expect("server identity should resolve");
    let path_client = TmuxClient::with_socket_path(&identity.socket_path);

    let initial = client.list_panes().expect("initial pane should list");
    assert_eq!(initial.len(), 1);
    assert_eq!(
        path_client
            .server_identity()
            .expect("socket-path identity should resolve"),
        identity
    );
    let static_id = initial[0].pane_id.clone();
    let first_static = client
        .capture_pane(&static_id)
        .expect("static pane should capture");
    thread::sleep(Duration::from_millis(100));
    let second_static = client
        .capture_pane(&static_id)
        .expect("static pane should recapture");
    assert_eq!(first_static, second_static);

    let changing_id = server.run(&[
        "split-window",
        "-d",
        "-P",
        "-F",
        "#{pane_id}",
        "-t",
        "test",
        "while :; do date +%s%N; sleep 0.05; done",
    ]);
    thread::sleep(Duration::from_millis(150));
    let first_changing = client
        .capture_pane(&changing_id)
        .expect("changing pane should capture");
    thread::sleep(Duration::from_millis(150));
    let second_changing = client
        .capture_pane(&changing_id)
        .expect("changing pane should recapture");
    assert_ne!(first_changing, second_changing);

    let burst_id = server.run(&[
        "split-window",
        "-d",
        "-P",
        "-F",
        "#{pane_id}",
        "-t",
        "test",
        "for i in 1 2 3 4 5; do echo \"$i\"; sleep 0.03; done; sleep 30",
    ]);
    thread::sleep(Duration::from_secs(1));
    let first_burst = client
        .capture_pane(&burst_id)
        .expect("completed burst should capture");
    thread::sleep(Duration::from_millis(250));
    let second_burst = client
        .capture_pane(&burst_id)
        .expect("completed burst should recapture");
    assert_eq!(first_burst, second_burst);
    assert_eq!(client.list_panes().expect("all panes should list").len(), 3);

    let window_id = initial[0].window_id.clone();
    let summary = WindowSummary {
        changing: 1,
        static_count: 1,
    }
    .render_tmux_segment(
        DEFAULT_TMUX_LABEL,
        DEFAULT_TMUX_SEPARATOR,
        DEFAULT_CHANGING_SYMBOL,
        DEFAULT_STATIC_SYMBOL,
    );
    client
        .set_window_summary(&window_id, &summary)
        .expect("summary should publish");
    assert_eq!(
        server.run(&[
            "display-message",
            "-p",
            "-t",
            &window_id,
            WINDOW_SUMMARY_FORMAT,
        ]),
        summary
    );

    server.run(&["kill-pane", "-t", &changing_id]);
    server.run(&["kill-pane", "-t", &burst_id]);
    assert_eq!(
        client
            .list_panes()
            .expect("remaining pane should list")
            .len(),
        1
    );
    client
        .clear_all_summaries()
        .expect("summaries should clear");
    assert_eq!(
        server.run(&[
            "display-message",
            "-p",
            "-t",
            &window_id,
            WINDOW_SUMMARY_FORMAT,
        ]),
        ""
    );
}

#[test]
fn watcher_is_singleton_per_server_and_exits_with_server() {
    let mut first_server = TestServer::start();
    let mut second_server = TestServer::start();
    let mut first_watcher = spawn_watcher(&first_server.socket);
    let mut second_watcher = spawn_watcher(&second_server.socket);
    thread::sleep(Duration::from_millis(250));

    assert!(
        first_watcher
            .try_wait()
            .expect("first watcher status should be readable")
            .is_none()
    );
    assert!(
        second_watcher
            .try_wait()
            .expect("second watcher status should be readable")
            .is_none()
    );

    let duplicate = Command::new(env!("CARGO_BIN_EXE_tmux-agent-watch"))
        .args([
            "--socket",
            &first_server.socket,
            "watch",
            "--no-tmux-status",
        ])
        .output()
        .expect("duplicate watcher should execute");
    assert!(duplicate.status.success());
    assert!(
        String::from_utf8_lossy(&duplicate.stderr)
            .contains("watcher already running")
    );

    first_server.kill();
    second_server.kill();
    wait_for_exit(&mut first_watcher);
    wait_for_exit(&mut second_watcher);
}
