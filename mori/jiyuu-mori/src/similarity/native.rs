//! Copy fingerprints from the graph, then score on the native worker pool.

use super::{BANDS, Entry, Index, SimilarEdge, fingerprint};
use std::{
    collections::HashSet,
    ffi::{CStr, CString, c_char, c_int, c_void},
    sync::{
        OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
};

type ReadNode = unsafe extern "C" fn(
    *mut c_void,
    i64,
    *const c_char,
    *const c_char,
    *const c_char,
);
type RunWorker = unsafe extern "C" fn(c_int, *mut c_void);

unsafe extern "C" {
    fn cbm_similarity_read_buffer(
        graph: *const c_void,
        data: *mut c_void,
        read: ReadNode,
    );
    fn cbm_similarity_hash_bands(fingerprint: *const u32, bands: *mut u16);
    fn cbm_similarity_run_workers(
        count: c_int,
        run: RunWorker,
        data: *mut c_void,
    );
    fn cbm_gbuf_insert_edge(
        graph: *mut c_void,
        source_id: i64,
        target_id: i64,
        kind: *const c_char,
        properties: *const c_char,
    ) -> i64;
}

unsafe fn copy_text(ptr: *const c_char) -> Option<Vec<u8>> {
    if ptr.is_null() {
        return None;
    }
    // SAFETY: the bridge provides a valid NUL-terminated string for the callback.
    Some(unsafe { CStr::from_ptr(ptr) }.to_bytes().to_vec())
}

unsafe extern "C" fn read_node(
    data: *mut c_void,
    id: i64,
    name: *const c_char,
    path: *const c_char,
    properties: *const c_char,
) {
    // SAFETY: this callback has exclusive access to the Rust vector and the
    // bridge supplies a non-null properties C string.
    let entries = unsafe { &mut *data.cast::<Vec<Entry>>() };
    let properties = unsafe { CStr::from_ptr(properties) };
    let Some(fingerprint) = fingerprint(properties.to_bytes()) else {
        return;
    };
    let mut bands = [0; BANDS];
    // SAFETY: these arrays provide the complete input and output of the C hash.
    unsafe {
        cbm_similarity_hash_bands(fingerprint.as_ptr(), bands.as_mut_ptr())
    };
    entries.push(Entry {
        id,
        // SAFETY: nullable strings supplied by the reader for this callback.
        name: unsafe { copy_text(name) },
        path: unsafe { copy_text(path) },
        fingerprint,
        bands,
    });
}

struct Work {
    index: Index,
    next_source: AtomicUsize,
    edges: Vec<OnceLock<Vec<SimilarEdge>>>,
}

unsafe extern "C" fn run_worker(worker: c_int, data: *mut c_void) {
    // SAFETY: the caller lends Work to all workers until the C pool joins.
    // Workers share the immutable index and use synchronized result slots.
    let work = unsafe { &*data.cast::<Work>() };
    let mut seen = HashSet::new();
    let mut edges = Vec::new();
    loop {
        let source = work.next_source.fetch_add(1, Ordering::Relaxed);
        if source >= work.index.entries.len() {
            break;
        }
        work.index.collect_edges(source, &mut seen, &mut edges);
    }
    let _ = work.edges[worker as usize].set(edges);
}

#[unsafe(no_mangle)]
unsafe extern "C" fn mori_similarity_buffer(
    graph: *mut c_void,
    workers: c_int,
) -> c_int {
    let mut entries = Vec::new();
    // SAFETY: the bridge owns the graph and reads it synchronously.
    unsafe {
        cbm_similarity_read_buffer(graph, (&raw mut entries).cast(), read_node);
    }
    if entries.len() < 2 {
        return 0;
    }
    let workers = workers.max(1);
    let mut work = Work {
        index: Index::new(entries),
        next_source: AtomicUsize::new(0),
        edges: (0..workers).map(|_| OnceLock::new()).collect(),
    };
    // SAFETY: this call joins every worker before we take ownership of the results.
    unsafe {
        cbm_similarity_run_workers(workers, run_worker, (&raw mut work).cast());
    }
    let mut count = 0;
    for edges in work.edges.into_iter().filter_map(OnceLock::into_inner) {
        for edge in edges {
            let score = edge.agreement as f64 / super::WORDS as f64;
            let properties = CString::new(format!(
                "{{\"jaccard\":{score:.3},\"same_file\":{}}}",
                edge.same_file
            ))
            .expect("the numeric score and boolean contain no NUL");
            // SAFETY: edge IDs belong to this graph. Insertion is now single
            // threaded and the graph copies both C strings.
            unsafe {
                cbm_gbuf_insert_edge(
                    graph,
                    edge.source,
                    edge.target,
                    c"SIMILAR_TO".as_ptr(),
                    properties.as_ptr(),
                );
            }
            count += 1;
        }
    }
    count
}
