use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use crate::model::{Activity, PaneInfo, PaneSnapshot, Snapshot};

#[derive(Debug)]
struct PaneRuntime {
    previous_capture: Vec<u8>,
    last_change_at: Instant,
    activity: Activity,
    width: u16,
    height: u16,
}

#[derive(Debug)]
pub struct Tracker {
    static_after: Duration,
    panes: HashMap<String, PaneRuntime>,
}

impl Tracker {
    pub fn new(static_after: Duration) -> Self {
        Self {
            static_after,
            panes: HashMap::new(),
        }
    }

    pub fn observe(
        &mut self,
        now: Instant,
        observed_at_ms: u64,
        tmux_server: String,
        observations: Vec<(PaneInfo, Vec<u8>)>,
    ) -> Snapshot {
        let observed_ids: HashSet<String> = observations
            .iter()
            .map(|(pane, _)| pane.pane_id.clone())
            .collect();
        self.panes
            .retain(|pane_id, _| observed_ids.contains(pane_id));

        let mut panes = Vec::with_capacity(observations.len());
        for (pane, capture) in observations {
            let runtime =
                self.panes.entry(pane.pane_id.clone()).or_insert_with(|| {
                    PaneRuntime {
                        previous_capture: capture.clone(),
                        last_change_at: now,
                        activity: Activity::Changing,
                        width: pane.width,
                        height: pane.height,
                    }
                });

            let resized =
                runtime.width != pane.width || runtime.height != pane.height;
            let changed = runtime.previous_capture != capture;
            if resized {
                runtime.previous_capture = capture;
                runtime.width = pane.width;
                runtime.height = pane.height;
            } else if changed {
                runtime.previous_capture = capture;
                runtime.last_change_at = now;
                runtime.activity = Activity::Changing;
            } else if now.duration_since(runtime.last_change_at)
                >= self.static_after
                && runtime.activity == Activity::Changing
            {
                runtime.activity = Activity::Static;
            }

            let agent = pane
                .agent_kind()
                .expect("tracker only accepts supported agent panes");
            let selected = pane.selected();
            panes.push(PaneSnapshot {
                pane_id: pane.pane_id,
                session_id: pane.session_id,
                session_name: pane.session_name,
                window_id: pane.window_id,
                window_index: pane.window_index,
                window_name: pane.window_name,
                pane_index: pane.pane_index,
                agent,
                title: pane.title,
                activity: runtime.activity.clone(),
                quiet_ms: now.duration_since(runtime.last_change_at).as_millis()
                    as u64,
                selected,
            });
        }

        panes.sort_by_key(|pane| {
            (
                pane.session_name.clone(),
                pane.window_index,
                pane.pane_index,
            )
        });
        Snapshot {
            schema_version: 1,
            observed_at_ms,
            tmux_server,
            panes,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(pane_id: &str, selected: bool) -> PaneInfo {
        PaneInfo {
            session_id: "$0".to_owned(),
            session_name: "dev".to_owned(),
            window_id: "@0".to_owned(),
            window_index: 0,
            window_name: "work".to_owned(),
            pane_id: pane_id.to_owned(),
            pane_index: 0,
            pane_tty: "/dev/pts/1".to_owned(),
            pane_pid: 123,
            current_command: "agent".to_owned(),
            window_active: selected,
            pane_active: selected,
            width: 80,
            height: 24,
            title: "Test agent".to_owned(),
        }
    }

    #[test]
    fn transitions_from_changing_to_static() {
        let start = Instant::now();
        let mut tracker = Tracker::new(Duration::from_secs(2));

        let first = tracker.observe(
            start,
            1,
            "test".to_owned(),
            vec![(pane("%1", false), b"same".to_vec())],
        );
        assert_eq!(first.panes[0].activity, Activity::Changing);

        tracker.observe(
            start + Duration::from_secs(1),
            2,
            "test".to_owned(),
            vec![(pane("%1", false), b"changed".to_vec())],
        );
        let second = tracker.observe(
            start + Duration::from_secs(4),
            3,
            "test".to_owned(),
            vec![(pane("%1", false), b"changed".to_vec())],
        );
        assert_eq!(second.panes[0].activity, Activity::Static);
    }

    #[test]
    fn initial_quiet_pane_becomes_static() {
        let start = Instant::now();
        let mut tracker = Tracker::new(Duration::from_secs(2));
        tracker.observe(
            start,
            1,
            "test".to_owned(),
            vec![(pane("%1", false), b"same".to_vec())],
        );

        let static_snapshot = tracker.observe(
            start + Duration::from_secs(3),
            2,
            "test".to_owned(),
            vec![(pane("%1", false), b"same".to_vec())],
        );
        assert_eq!(static_snapshot.panes[0].activity, Activity::Static);
    }

    #[test]
    fn capture_change_resets_static_timer() {
        let start = Instant::now();
        let mut tracker = Tracker::new(Duration::from_secs(1));
        tracker.observe(
            start,
            1,
            "test".to_owned(),
            vec![(pane("%1", false), b"first".to_vec())],
        );
        tracker.observe(
            start + Duration::from_secs(2),
            2,
            "test".to_owned(),
            vec![(pane("%1", false), b"first".to_vec())],
        );

        let changed = tracker.observe(
            start + Duration::from_secs(3),
            3,
            "test".to_owned(),
            vec![(pane("%1", false), b"second".to_vec())],
        );
        assert_eq!(changed.panes[0].activity, Activity::Changing);

        let selected = tracker.observe(
            start + Duration::from_secs(5),
            4,
            "test".to_owned(),
            vec![(pane("%1", true), b"second".to_vec())],
        );
        assert_eq!(selected.panes[0].activity, Activity::Static);
    }

    #[test]
    fn resize_resets_capture_baseline_without_reporting_activity() {
        let start = Instant::now();
        let mut tracker = Tracker::new(Duration::from_secs(1));
        tracker.observe(
            start,
            1,
            "test".to_owned(),
            vec![(pane("%1", false), b"before resize".to_vec())],
        );
        let static_snapshot = tracker.observe(
            start + Duration::from_secs(2),
            2,
            "test".to_owned(),
            vec![(pane("%1", false), b"before resize".to_vec())],
        );
        assert_eq!(static_snapshot.panes[0].activity, Activity::Static);

        let mut resized_pane = pane("%1", false);
        resized_pane.width = 120;
        let resized_snapshot = tracker.observe(
            start + Duration::from_secs(3),
            3,
            "test".to_owned(),
            vec![(resized_pane, b"reflowed capture".to_vec())],
        );
        assert_eq!(resized_snapshot.panes[0].activity, Activity::Static);
        assert_eq!(resized_snapshot.panes[0].quiet_ms, 3_000);

        let mut stable_pane = pane("%1", false);
        stable_pane.width = 120;
        let stable_snapshot = tracker.observe(
            start + Duration::from_secs(4),
            4,
            "test".to_owned(),
            vec![(stable_pane, b"reflowed capture".to_vec())],
        );
        assert_eq!(stable_snapshot.panes[0].activity, Activity::Static);
    }

    #[test]
    fn removes_panes_missing_from_latest_observation() {
        let start = Instant::now();
        let mut tracker = Tracker::new(Duration::from_secs(1));
        tracker.observe(
            start,
            1,
            "test".to_owned(),
            vec![(pane("%1", false), Vec::new())],
        );

        let empty = tracker.observe(start, 2, "test".to_owned(), Vec::new());
        assert!(empty.panes.is_empty());
        assert!(tracker.panes.is_empty());
    }
}
