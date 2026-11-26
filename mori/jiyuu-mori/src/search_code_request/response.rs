use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::ffi::{CStr, CString};
use std::fmt::Write;

use serde::Serialize;
use serde::ser::{SerializeMap, Serializer};
use serde_json::{
    Value, json,
    value::{RawValue, to_raw_value},
};

use super::native::{GatheredData, Native, RawMatch, SearchResult};
use super::{Mode, PageRequest, Request};

const RESULT_COLUMNS: [&CStr; 8] = [
    c"qn",
    c"label",
    c"file",
    c"lines",
    c"matches",
    c"matches_omitted",
    c"in",
    c"out",
];
const RAW_COLUMNS: [&CStr; 11] = [
    c"file",
    c"line",
    c"content",
    c"content_start_byte",
    c"content_returned_bytes",
    c"content_total_bytes",
    c"match_start_byte",
    c"match_end_byte",
    c"content_has_more",
    c"content_next_offset",
    c"match_fully_returned",
];
const PIPE_WARNING: &str = "pattern contains '|' but regex=false, so it is matched literally (not as alternation). Pass regex=true for 'foo|bar' to mean 'foo OR bar'.";
const PATH_HINT: &str = "no results; a file_pattern with '/' must match the whole repo-relative path (e.g. src/**/*.c); to scope by folder you can also use path_filter (regex on the repo-relative path, e.g. ^src/cli/)";

#[derive(Default)]
struct Object(Vec<(String, Box<RawValue>)>);

impl Object {
    fn push(&mut self, key: impl Into<String>, value: impl Serialize) {
        self.0.push((
            key.into(),
            to_raw_value(&value).expect("serializable search field"),
        ));
    }

    fn json(&self) -> CString {
        cstring(serde_json::to_vec(self).expect("serializable search response"))
    }
}

impl Serialize for Object {
    fn serialize<S: Serializer>(
        &self,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (key, value) in &self.0 {
            map.serialize_entry(key, value)?;
        }
        map.end()
    }
}

fn cstring(bytes: impl Into<Vec<u8>>) -> CString {
    CString::new(bytes).expect("search payload contains no interior NUL")
}

// Mirror cbm_output_encode_text at the JSON boundary. The shared tree
// table writer takes original bytes and applies this encoding itself.
fn encode(text: &CStr) -> Cow<'_, str> {
    match text.to_str() {
        Ok(text)
            if text.starts_with("@bytes:") || text.starts_with("@utf8:") =>
        {
            Cow::Owned(format!("@utf8:{text}"))
        }
        Ok(text) => Cow::Borrowed(text),
        Err(_) => {
            let mut encoded = String::from("@bytes:");
            for byte in text.to_bytes() {
                write!(&mut encoded, "{byte:02x}")
                    .expect("writing to String is infallible");
            }
            Cow::Owned(encoded)
        }
    }
}

fn score(row: &SearchResult) -> i32 {
    let label = row.label();
    let label_score = if label == c"Function" || label == c"Method" {
        10
    } else if label == c"Route" {
        15
    } else {
        0
    };
    let file = row.file().to_bytes();
    let has =
        |needle: &[u8]| file.windows(needle.len()).any(|part| part == needle);
    row.in_degree
        .wrapping_add(label_score)
        .wrapping_sub(
            if has(b"vendored/") || has(b"vendor/") || has(b"node_modules/") {
                50
            } else {
                0
            },
        )
        .wrapping_sub(if has(b"test") || has(b"spec") { 5 } else { 0 })
}

fn lines(row: &SearchResult) -> String {
    if row.start_line > 0 {
        format!("{}-{}", row.start_line, row.end_line.max(row.start_line))
    } else {
        String::new()
    }
}

#[derive(Clone, Copy)]
struct Page {
    start: usize,
    count: usize,
    limit: usize,
    total: usize,
}

impl Page {
    fn new(request: PageRequest, total: usize) -> Self {
        let start = request.offset.min(total);
        Self {
            start,
            count: (total - start).min(request.limit),
            limit: request.limit,
            total,
        }
    }

    fn end(self) -> usize {
        self.start + self.count
    }

    fn more(self) -> bool {
        self.end() < self.total
    }

    fn continuation(
        self,
        output: &mut Object,
        more: &str,
        next: &str,
        positive_limit: &str,
        higher_budget: Option<&str>,
    ) {
        output.push(more, self.more());
        if self.more() {
            if self.count > 0 {
                output.push(next, self.end());
            } else if self.limit == 0 {
                output.push(positive_limit, true);
            } else if let Some(higher_budget) = higher_budget {
                output.push(higher_budget, true);
            }
        }
    }
}

#[derive(Clone, Copy)]
struct Selection {
    results: Page,
    raw: Page,
    directories: Page,
    source_lines: i32,
    budget_hit: bool,
}

pub(super) struct Response<'a> {
    req: &'a Request,
    data: &'a GatheredData,
    native: Native<'a>,
    root: &'a CStr,
    rows: Vec<&'a SearchResult>,
    directories: Vec<(CString, usize)>,
    pub elapsed_ms: u64,
    pub enrich_ms: u64,
    pub warn_literal_pipe: bool,
    pub hint_path_filter: bool,
}

impl<'a> Response<'a> {
    pub fn new(
        req: &'a Request,
        data: &'a GatheredData,
        native: Native<'a>,
        root: &'a CStr,
    ) -> Self {
        Self {
            req,
            data,
            native,
            root,
            rows: data.results().iter().collect(),
            directories: Vec::new(),
            elapsed_ms: 0,
            enrich_ms: data.enrich_ms,
            warn_literal_pipe: false,
            hint_path_filter: false,
        }
    }

    pub fn rank(&mut self) {
        // Cache the score so the sort never rescans a path for each comparison.
        self.rows.sort_by_cached_key(|&row| {
            (
                std::cmp::Reverse(score(row)),
                row.qualified_name(),
                row.file(),
                row.start_line,
                row.end_line,
            )
        });
    }

    fn gather_directories(&mut self) {
        let mut indices = HashMap::new();
        for row in &self.rows {
            let file = row.file().to_bytes();
            let end = file
                .iter()
                .position(|&byte| byte == b'/')
                .map_or(file.len(), |i| i + 1);
            let directory = &file[..end];
            let index = *indices.entry(directory).or_insert_with(|| {
                self.directories.push((cstring(directory), 0));
                self.directories.len() - 1
            });
            self.directories[index].1 += 1;
        }
    }

    fn selection(&self) -> Selection {
        let mut raw = Page::new(self.req.raw, self.data.raw_count as usize);
        raw.count = self.data.raw().len();
        Selection {
            results: Page::new(self.req.results, self.rows.len()),
            raw,
            directories: Page::new(self.req.directories, self.directories.len()),
            source_lines: self.req.source_max_lines,
            budget_hit: false,
        }
    }

    #[cfg(feature = "native-engine-test-seams")]
    pub fn render_fixture(mut self) -> Option<CString> {
        self.gather_directories();
        self.render(self.selection())
    }

    pub fn with_budget(mut self) -> Option<CString> {
        self.gather_directories();
        let budget = self.req.byte_budget;
        let over = |payload: &Option<CString>| {
            payload
                .as_ref()
                .is_some_and(|payload| payload.count_bytes() > budget)
        };
        let mut selected = self.selection();
        let mut payload = self.render(selected);
        if over(&payload) {
            selected.budget_hit = true;
            payload = self.render(selected);
        }

        // Drop whole raw rows, then directory rows, before ranked results.
        while over(&payload) && selected.raw.count > 0 {
            selected.raw.count -= 1;
            payload = self.render(selected);
        }
        while over(&payload) && selected.directories.count > 0 {
            selected.directories.count -= 1;
            payload = self.render(selected);
        }
        if over(&payload)
            && self.req.mode == Mode::Full
            && selected.source_lines > 0
        {
            let mut low = 0;
            let mut high = selected.source_lines - 1;
            let mut best = None;
            while low <= high {
                selected.source_lines = low + (high - low) / 2;
                let candidate = self.render(selected);
                if candidate.is_some() && !over(&candidate) {
                    best = Some((selected.source_lines, candidate));
                    low = selected.source_lines + 1;
                } else {
                    high = selected.source_lines - 1;
                }
            }
            if let Some((lines, candidate)) = best {
                selected.source_lines = lines;
                payload = candidate;
            } else {
                selected.source_lines = 0;
                payload = self.render(selected);
            }
        }
        if over(&payload) && selected.results.count > 0 {
            // Prefix factoring can make a longer row prefix cheaper.
            payload = (0..selected.results.count).rev().find_map(|count| {
                selected.results.count = count;
                let candidate = self.render(selected);
                if over(&candidate) { None } else { candidate }
            });
        }
        if payload.is_none() || over(&payload) {
            let floor = self.floor(selected).json();
            return self.project(floor, self.req.json);
        }
        payload
    }

    fn project(&self, json: CString, direct: bool) -> Option<CString> {
        if direct {
            Some(json)
        } else {
            self.native.json_tree(&json).map(|tree| tree.to_owned())
        }
    }

    fn render(&self, selected: Selection) -> Option<CString> {
        if self.req.mode == Mode::Compact && !self.req.json {
            self.compact(selected)
        } else {
            self.project(self.json(selected), self.req.json)
        }
    }

    fn selected_rows(&self, selected: Selection) -> &[&SearchResult] {
        &self.rows[selected.results.start..selected.results.end()]
    }

    fn selected_dirs(&self, selected: Selection) -> &[(CString, usize)] {
        &self.directories[selected.directories.start..selected.directories.end()]
    }

    fn selected_raw(&self, selected: Selection) -> &[RawMatch] {
        &self.data.raw()[..selected.raw.count]
    }

    fn summary(&self, output: &mut Object, selected: Selection) {
        let Selection {
            results,
            raw,
            directories,
            ..
        } = selected;
        output.push("total_grep_matches", self.data.grep_count);
        output.push("total_results", results.total);
        output.push("raw_match_count", raw.total);
        output.push("total_relation", "eq");
        output.push("result_offset", results.start);
        output.push("results_returned", results.count);
        result_continuation(output, results);
        output.push("raw_returned", raw.count);
        raw.continuation(
            output,
            "raw_has_more",
            "raw_next_offset",
            "raw_continuation_requires_positive_limit",
            Some("raw_continuation_requires_higher_budget"),
        );
        if self.data.raw_content_truncated > 0 {
            output
                .push("raw_content_truncated", self.data.raw_content_truncated);
        }
        output.push("directories_total", directories.total);
        output.push("directories_returned", directories.count);
        directories.continuation(
            output,
            "directories_has_more",
            "directory_next_offset",
            "directory_continuation_requires_positive_limit",
            Some("directory_continuation_requires_higher_budget"),
        );
        output.push(
            "truncated",
            results.more()
                || raw.more()
                || directories.more()
                || selected.budget_hit,
        );
        if selected.budget_hit {
            output.push("truncation_reason", "output_budget");
        }
        if self.req.mode == Mode::Full {
            output.push("source_max_lines_returned", selected.source_lines);
        }
        if self.req.debug {
            output.push("scope_ms", self.data.scope_ms);
            output.push("scan_ms", self.data.scan_ms);
            output.push("enrich_ms", self.enrich_ms);
        }
        output.push("elapsed_ms", self.elapsed_ms);
    }

    fn floor(&self, selected: Selection) -> Object {
        let Selection {
            mut results,
            mut raw,
            mut directories,
            ..
        } = selected;
        results.count = 0;
        raw.count = 0;
        directories.count = 0;
        let mut output = Object::default();
        output.push("total_results", results.total);
        output.push("raw_match_count", raw.total);
        output.push("result_offset", results.start);
        output.push("results_returned", 0);
        result_continuation(&mut output, results);
        output.push("raw_offset", raw.start);
        output.push("raw_returned", 0);
        raw.continuation(
            &mut output,
            "raw_has_more",
            "raw_next_offset",
            "raw_continuation_requires_positive_limit",
            None,
        );
        output.push("directories_total", directories.total);
        output.push("directory_offset", directories.start);
        output.push("directories_returned", 0);
        directories.continuation(
            &mut output,
            "directories_has_more",
            "directory_next_offset",
            "directory_continuation_requires_positive_limit",
            None,
        );
        output.push("truncated", true);
        output.push("truncation_reason", "output_budget");
        output
    }

    fn json(&self, selected: Selection) -> CString {
        let mut output = Object::default();
        if self.req.mode == Mode::Files {
            let files = self
                .selected_rows(selected)
                .iter()
                .map(|row| row.file())
                .chain(self.selected_raw(selected).iter().map(RawMatch::file));
            let mut seen = HashSet::new();
            output.push(
                "files",
                files
                    .filter(|file| seen.insert(*file))
                    .map(encode)
                    .collect::<Vec<_>>(),
            );
        } else {
            let mut columns = column_names(RESULT_COLUMNS);
            let detail = match self.req.mode {
                Mode::Full => Some("source"),
                Mode::Compact if self.req.context > 0 => Some("context"),
                _ => None,
            };
            columns.extend(detail);
            output.push("cols", columns);
            let rows: Vec<_> = self
                .selected_rows(selected)
                .iter()
                .map(|row| {
                    let matches = shown_matches(row, self.req.match_limit);
                    let mut cells = vec![
                        json!(encode(row.qualified_name())),
                        json!(encode(row.label())),
                        json!(encode(row.file())),
                        json!(lines(row)),
                        json!(matches),
                        json!(row.match_count as usize - matches.len()),
                        json!(row.in_degree),
                        json!(row.out_degree),
                    ];
                    if detail.is_some() {
                        cells.push(
                            self.source_detail(row, selected.source_lines),
                        );
                    }
                    cells
                })
                .collect();
            output.push("rows", rows);
            output.push("raw_matches", json!({
                "cols": column_names(RAW_COLUMNS),
                "rows": self.selected_raw(selected).iter().map(raw_json).collect::<Vec<_>>()
            }));
        }

        let mut directories = Object::default();
        for (directory, count) in self.selected_dirs(selected) {
            directories.push(encode(directory), count);
        }
        output.push("directories", directories);
        self.summary(&mut output, selected);
        if !self.rows.is_empty() && self.data.grep_count > 0 {
            let ratio = self.data.grep_count as f64
                / (self.rows.len() + self.data.raw_count as usize) as f64;
            output.push("dedup_ratio", format!("{ratio:.1}x"));
        }
        let mut warnings = Vec::new();
        if self.warn_literal_pipe {
            warnings.push(PIPE_WARNING.to_owned());
        }
        if self.elapsed_ms >= 5000 {
            warnings.push(format!("search took {}ms (>5s); narrow file_pattern/path_filter or use a more specific pattern", self.elapsed_ms as i32));
            self.native.log_slow(self.elapsed_ms);
        }
        if !warnings.is_empty() {
            output.push("warnings", warnings);
        }
        if self.hint_path_filter {
            output.push("hint", PATH_HINT);
        }
        output.json()
    }

    fn source_detail(&self, row: &SearchResult, source_lines: i32) -> Value {
        if row.start_line <= 0 || row.end_line <= 0 {
            return json!({});
        }
        if self.req.mode == Mode::Full {
            if source_lines == 0 {
                return match self.native.source(self.root, row.file(), 0, 0) {
                    Some(_) => json!({"source_omitted": true}),
                    None => json!({}),
                };
            }
            let mut start = row.start_line;
            let mut end = row.end_line;
            let truncated = end - start + 1 > source_lines;
            if truncated {
                if let Some(&first) = row.matches().first() {
                    start = start.max(first.saturating_sub(5));
                }
                end = end.min(start.saturating_add(source_lines - 1));
            }
            match self.native.source(self.root, row.file(), start, end) {
                Some(source) if truncated => json!({
                    "source": encode(&source), "source_start": start, "source_truncated": true
                }),
                Some(source) => json!({"source": encode(&source)}),
                None => json!({}),
            }
        } else if let Some(&first) = row.matches().first() {
            let start = first.saturating_sub(self.req.context).max(1);
            let end = row.last_match_line.saturating_add(self.req.context);
            match self.native.source(self.root, row.file(), start, end) {
                Some(context) => {
                    json!({"context": encode(&context), "context_start": start})
                }
                None => json!({}),
            }
        } else {
            json!({})
        }
    }

    fn compact(&self, selected: Selection) -> Option<CString> {
        let mut payload = Vec::new();
        let results = self
            .selected_rows(selected)
            .iter()
            .map(|row| {
                let matches = shown_matches(row, self.req.match_limit);
                [
                    row.qualified_name().to_owned(),
                    row.label().to_owned(),
                    row.file().to_owned(),
                    cstring(lines(row)),
                    cstring(
                        matches
                            .iter()
                            .map(i32::to_string)
                            .collect::<Vec<_>>()
                            .join(";"),
                    ),
                    number(row.match_count as usize - matches.len()),
                    number(row.in_degree),
                    number(row.out_degree),
                ]
            })
            .collect();
        payload.extend_from_slice(
            self.native
                .tree_table(
                    c"results",
                    RESULT_COLUMNS,
                    results,
                    [true, true, true, true, true, false, false, false],
                    [true, false, true, false, false, false, false, false],
                )?
                .to_bytes(),
        );
        if selected.raw.count > 0 {
            payload.extend_from_slice(
                self.native
                    .tree_table(
                        c"raw",
                        RAW_COLUMNS,
                        self.selected_raw(selected)
                            .iter()
                            .map(raw_tree)
                            .collect(),
                        [
                            true, false, true, false, false, false, false,
                            false, false, false, false,
                        ],
                        [
                            true, false, false, false, false, false, false,
                            false, false, false, false,
                        ],
                    )?
                    .to_bytes(),
            );
        }
        if selected.directories.count > 0 {
            let directories = self
                .selected_dirs(selected)
                .iter()
                .map(|(dir, count)| [dir.to_owned(), number(count)])
                .collect();
            payload.extend_from_slice(
                self.native
                    .tree_table(
                        c"directories",
                        [c"dir", c"hits"],
                        directories,
                        [true, false],
                        [true, false],
                    )?
                    .to_bytes(),
            );
        }
        let mut summary = Object::default();
        self.summary(&mut summary, selected);
        if self.warn_literal_pipe {
            summary.push("warning", PIPE_WARNING);
        }
        if self.hint_path_filter {
            summary.push("hint", PATH_HINT);
        }
        if self.elapsed_ms >= 5000 {
            summary.push("warning_slow", "search was slow; narrow file_pattern/path_filter or use a more specific pattern");
        }
        payload.extend_from_slice(
            self.native.json_tree(&summary.json())?.to_bytes(),
        );
        Some(cstring(payload))
    }
}

fn result_continuation(output: &mut Object, results: Page) {
    results.continuation(
        output,
        "has_more",
        "next_offset",
        "result_continuation_requires_positive_limit",
        Some("result_continuation_requires_higher_budget"),
    );
}

fn column_names<const N: usize>(
    columns: [&'static CStr; N],
) -> Vec<&'static str> {
    columns
        .into_iter()
        .map(|name| name.to_str().unwrap())
        .collect()
}

fn shown_matches(row: &SearchResult, limit: usize) -> &[i32] {
    &row.matches()[..row.matches().len().min(limit)]
}

fn content_next(row: &RawMatch) -> Option<usize> {
    let end = row.content_start_byte + row.content_returned_bytes;
    (end < row.content_total_bytes).then_some(end)
}

fn match_returned(row: &RawMatch) -> bool {
    row.match_known
        && row.content_start_byte <= row.match_start_byte
        && row.content_start_byte + row.content_returned_bytes
            >= row.match_end_byte
}

fn raw_json(row: &RawMatch) -> Value {
    let next = content_next(row);
    json!([
        encode(row.file()),
        row.line,
        encode(row.content()),
        row.content_start_byte,
        row.content_returned_bytes,
        row.content_total_bytes,
        row.match_known.then_some(row.match_start_byte),
        row.match_known.then_some(row.match_end_byte),
        next.is_some(),
        next,
        match_returned(row)
    ])
}

fn number(value: impl ToString) -> CString {
    cstring(value.to_string())
}

fn raw_tree(row: &RawMatch) -> [CString; 11] {
    let next = content_next(row);
    let (match_start, match_end) = if row.match_known {
        (number(row.match_start_byte), number(row.match_end_byte))
    } else {
        (CString::default(), CString::default())
    };
    [
        row.file().to_owned(),
        number(row.line),
        row.content().to_owned(),
        number(row.content_start_byte),
        number(row.content_returned_bytes),
        number(row.content_total_bytes),
        match_start,
        match_end,
        number(next.is_some()),
        next.map(number).unwrap_or_default(),
        number(match_returned(row)),
    ]
}
