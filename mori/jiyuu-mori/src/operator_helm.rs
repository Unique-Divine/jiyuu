//! Resolve Go chart load sites to repository Charts and emit DEPLOYS edges.

use std::collections::HashMap;
use std::ffi::{CStr, CString, c_char, c_int};
use std::ptr;

// The Go expression evaluator and the previous matcher use CBM_SZ_1K buffers.
const PATH_BYTES: usize = 1023;
const CHART_BASENAME_BYTES: usize = 255;

#[repr(C)]
struct LinkRun {
    _opaque: [u8; 0],
}

#[repr(C)]
struct Site {
    function_id: i64,
    line: c_int,
    tail: *const c_char,
    root: *const c_char,
}

#[repr(C)]
struct ChartNode {
    id: i64,
    path: *const c_char,
}

#[repr(C)]
struct LinkOps {
    chart_at: unsafe extern "C" fn(*const LinkRun, c_int) -> ChartNode,
    cancelled: unsafe extern "C" fn(*const LinkRun) -> bool,
    emit: unsafe extern "C" fn(
        *mut LinkRun,
        &Site,
        i64,
        *const c_char,
        *const c_char,
    ),
}

struct Chart<'a> {
    id: i64,
    dir: &'a [u8],
}

struct Charts<'a> {
    by_basename: HashMap<&'a [u8], Vec<Chart<'a>>>,
    candidates: &'a mut u64,
}

fn split_path(path: &[u8]) -> (&[u8], &[u8]) {
    path.iter()
        .rposition(|&byte| byte == b'/')
        .map_or((&[][..], path), |slash| {
            (&path[..slash], &path[slash + 1..])
        })
}

fn bounded(path: &[u8], max: usize) -> &[u8] {
    &path[..path.len().min(max)]
}

fn normalized(mut tail: &[u8]) -> &[u8] {
    while let Some(rest) = tail.strip_prefix(b"./") {
        tail = rest;
    }
    let start = tail
        .iter()
        .position(|&byte| byte != b'/')
        .unwrap_or(tail.len());
    tail = bounded(&tail[start..], PATH_BYTES);
    let end = tail
        .iter()
        .rposition(|&byte| byte != b'/')
        .map_or(0, |i| i + 1);
    &tail[..end]
}

impl<'a> Charts<'a> {
    fn add(&mut self, id: i64, path: &'a [u8]) {
        let (dir, file) = split_path(path);
        if dir.is_empty() || !matches!(file, b"Chart.yaml" | b"Chart.yml") {
            return;
        }
        let (_, basename) = split_path(dir);
        self.by_basename
            .entry(bounded(basename, CHART_BASENAME_BYTES))
            .or_default()
            .push(Chart { id, dir });
    }

    fn unique(&mut self, tail: &[u8]) -> Option<(i64, &'a [u8])> {
        let tail = normalized(tail);
        if tail.is_empty() {
            return None;
        }
        let (_, basename) = split_path(tail);
        let mut hit = None;
        for chart in self.by_basename.get(basename)? {
            *self.candidates += 1;
            if let Some(prefix) = chart.dir.strip_suffix(tail) {
                if !prefix.is_empty() && !prefix.ends_with(b"/") {
                    continue;
                }
                if hit.is_some() {
                    return None;
                }
                hit = Some((chart.id, bounded(chart.dir, PATH_BYTES)));
            }
        }
        hit
    }
}

// A unique suffix can locate an unknown root. A conflicting root contributes
// no evidence to ambiguous sites, but each unique suffix still emits its edge.
// SAFETY: The paths carried by sites must remain valid for the call.
unsafe fn link_sites<'a>(
    sites: &'a [Site],
    charts: &mut Charts<'a>,
    cancelled: impl Fn() -> bool,
    mut emit: impl FnMut(&Site, i64, &[u8], Option<&[u8]>),
) -> c_int {
    let mut roots = HashMap::<&[u8], Option<&[u8]>>::new();
    for site in sites {
        if cancelled() {
            return 0;
        }
        // SAFETY: The native caller lends these NUL-terminated paths with the
        // site slice. An absent tail can result from a C allocation failure.
        let (Some(root), Some(tail)) =
            (unsafe { (path_bytes(site.root), path_bytes(site.tail)) })
        else {
            continue;
        };
        if let Some((_, dir)) = charts.unique(tail) {
            let prefix_len =
                dir.len().saturating_sub(normalized(tail).len() + 1);
            let prefix = &dir[..prefix_len];
            let known = roots.entry(root).or_insert(Some(prefix));
            if *known != Some(prefix) {
                *known = None;
            }
        }
    }

    let mut edges = 0;
    for site in sites {
        if cancelled() {
            break;
        }
        // SAFETY: Same borrowed paths as in the evidence pass above.
        let Some(tail) = (unsafe { path_bytes(site.tail) }) else {
            continue;
        };
        let matched = match charts.unique(tail) {
            Some((id, dir)) => Some((id, dir, None)),
            None => unsafe { path_bytes(site.root) }
                .and_then(|root| roots.get(root).copied().flatten())
                // The C matcher reserved this directory spelling for conflicts.
                .filter(|prefix| *prefix != b"\x01")
                .and_then(|prefix| {
                    let mut full = prefix.to_vec();
                    if !prefix.is_empty() {
                        full.push(b'/');
                    }
                    full.extend_from_slice(tail);
                    let want = bounded(&full, PATH_BYTES);
                    let (id, dir) = charts.unique(want)?;
                    (dir == want).then_some((id, dir, Some(prefix)))
                }),
        };
        if let Some((id, dir, root)) = matched {
            if cancelled() {
                break;
            }
            emit(site, id, dir, root);
            edges += 1;
        }
    }
    edges
}

// SAFETY: The caller owns the pointed-to paths for the returned lifetime.
unsafe fn path_bytes<'a>(path: *const c_char) -> Option<&'a [u8]> {
    (!path.is_null()).then(|| unsafe { CStr::from_ptr(path).to_bytes() })
}

#[cfg_attr(feature = "native-engine-abi", unsafe(no_mangle))]
unsafe extern "C" fn cbm_rust_operator_helm_deploys(
    run: *mut LinkRun,
    ops: &LinkOps,
    chart_count: c_int,
    sites: *const Site,
    site_count: c_int,
    candidates: &mut u64,
) -> c_int {
    if site_count <= 0 {
        return 0;
    }
    let mut charts = Charts {
        by_basename: HashMap::new(),
        candidates,
    };
    // SAFETY: C supplies a live run and callbacks, and lends the site array
    // and immutable graph-owned paths for this call. Emission only adds edges.
    unsafe {
        let cancelled = || (ops.cancelled)(run);
        let sites = std::slice::from_raw_parts(sites, site_count as usize);
        for index in 0..chart_count {
            if cancelled() {
                return 0;
            }
            let node = (ops.chart_at)(run, index);
            if let Some(path) = path_bytes(node.path) {
                charts.add(node.id, path);
            }
        }
        link_sites(sites, &mut charts, cancelled, |site, id, dir, root| {
            // These slices come from C strings and cannot contain NUL.
            let dir = CString::new(dir).expect("chart directory from C string");
            let root = root
                .map(|dir| CString::new(dir).expect("chart root from C string"));
            (ops.emit)(
                run,
                site,
                id,
                dir.as_ptr(),
                root.as_ref().map_or(ptr::null(), |dir| dir.as_ptr()),
            );
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq)]
    struct Edge(i64, i64, c_int, Vec<u8>, Option<Vec<u8>>);

    #[derive(Default)]
    struct Fixture {
        charts: Vec<ChartNode>,
        edges: Vec<Edge>,
        candidates: u64,
        cancel_after: Option<usize>,
    }

    unsafe extern "C" fn chart_at(
        run: *const LinkRun,
        index: c_int,
    ) -> ChartNode {
        let fixture = unsafe { &*run.cast::<Fixture>() };
        let chart = &fixture.charts[index as usize];
        ChartNode {
            id: chart.id,
            path: chart.path,
        }
    }

    unsafe extern "C" fn cancelled(run: *const LinkRun) -> bool {
        let fixture = unsafe { &*run.cast::<Fixture>() };
        fixture.cancel_after == Some(fixture.edges.len())
    }

    unsafe extern "C" fn emit(
        run: *mut LinkRun,
        site: &Site,
        id: i64,
        dir: *const c_char,
        root: *const c_char,
    ) {
        let fixture = unsafe { &mut *run.cast::<Fixture>() };
        fixture.edges.push(Edge(
            site.function_id,
            id,
            site.line,
            unsafe { CStr::from_ptr(dir) }.to_bytes().to_vec(),
            unsafe { path_bytes(root) }.map(<[u8]>::to_vec),
        ));
    }

    impl Fixture {
        fn new(charts: &[&'static CStr]) -> Self {
            Self {
                charts: charts
                    .iter()
                    .enumerate()
                    .map(|(i, path)| ChartNode {
                        id: i as i64 + 100,
                        path: path.as_ptr(),
                    })
                    .collect(),
                ..Self::default()
            }
        }

        fn run(&mut self, sites: &[Site]) -> c_int {
            let ops = LinkOps {
                chart_at,
                cancelled,
                emit,
            };
            let mut candidates = 0;
            let count = unsafe {
                cbm_rust_operator_helm_deploys(
                    (self as *mut Self).cast(),
                    &ops,
                    self.charts.len() as c_int,
                    sites.as_ptr(),
                    sites.len() as c_int,
                    &mut candidates,
                )
            };
            self.candidates = candidates;
            count
        }
    }

    fn site(
        function_id: i64,
        line: c_int,
        tail: &'static CStr,
        root: Option<&'static CStr>,
    ) -> Site {
        Site {
            function_id,
            line,
            tail: tail.as_ptr(),
            root: root.map_or(ptr::null(), CStr::as_ptr),
        }
    }

    #[test]
    fn chart_suffixes_and_inferred_roots_preserve_edge_order_and_properties() {
        let mut fixture = Fixture::new(&[
            c"charts/monitoring/exporter/Chart.yml",
            c"charts/minio/Chart.yaml",
            c"other/minio/Chart.yaml",
            c"third/not-minio/Chart.yaml",
            c"bogus/minio/values.yaml",
            c"Chart.yaml",
        ]);
        let root = Some(c"opt.ChartsDirectory");
        let sites = [
            site(1, 12, c"minio", root),
            site(2, 20, c"././monitoring/exporter///", root),
            site(3, 22, c"minio", None),
            site(4, 40, c"/charts/minio/", None),
        ];
        assert_eq!(fixture.run(&sites), 3);
        assert_eq!(
            fixture.edges,
            [
                Edge(
                    1,
                    101,
                    12,
                    b"charts/minio".to_vec(),
                    Some(b"charts".to_vec())
                ),
                Edge(2, 100, 20, b"charts/monitoring/exporter".to_vec(), None),
                Edge(4, 101, 40, b"charts/minio".to_vec(), None),
            ],
        );
        assert_eq!(fixture.candidates, 12);
    }

    #[test]
    fn conflicting_roots_leave_ambiguous_sites_unlinked() {
        let mut fixture = Fixture::new(&[
            c"charts/first/Chart.yaml",
            c"other/second/Chart.yaml",
            c"charts/minio/Chart.yaml",
            c"other/minio/Chart.yaml",
        ]);
        let root = Some(c"dir");
        let sites = [
            site(1, 1, c"first", root),
            site(2, 2, c"second", root),
            site(3, 3, c"first", root),
            site(4, 4, c"minio", root),
        ];
        assert_eq!(fixture.run(&sites), 3);
        assert_eq!(
            fixture.edges,
            [
                Edge(1, 100, 1, b"charts/first".to_vec(), None),
                Edge(2, 101, 2, b"other/second".to_vec(), None),
                Edge(3, 100, 3, b"charts/first".to_vec(), None),
            ],
        );
    }

    #[test]
    fn cancellation_stops_indexing_and_emission() {
        let charts = [c"charts/one/Chart.yaml", c"charts/two/Chart.yaml"];
        let sites = [site(1, 1, c"one", None), site(2, 2, c"two", None)];
        let mut fixture = Fixture::new(&charts);
        fixture.cancel_after = Some(0);
        assert_eq!(fixture.run(&sites), 0);
        assert!(fixture.edges.is_empty());
        assert_eq!(fixture.candidates, 0);

        fixture.cancel_after = Some(1);
        assert_eq!(fixture.run(&sites), 1);
        assert_eq!(
            fixture.edges,
            [Edge(1, 100, 1, b"charts/one".to_vec(), None)]
        );
        assert_eq!(fixture.candidates, 1);
    }
}
