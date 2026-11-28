use super::*;

const FIXTURE_KEY_MAX: usize = 4096;
const DEFINE_DEPTH_MAX: usize = 4;
const TOKEN_MAX: usize = 256;

#[derive(Clone, Copy)]
pub(super) struct Role<'a> {
    literal: &'a [u8],
    id: Id,
    reached: bool,
    scanned: bool,
    named: bool,
    covered: bool,
    started_by: Option<usize>,
    starters: (usize, usize),
}

struct RoleScan<'a> {
    handlers: &'a [Id],
    sinks: &'a [Id],
    starters: &'a mut Scratch<Id>,
    first_starter: usize,
}

struct LiteralScan<'a> {
    seeds: &'a mut Scratch<Id>,
    cases: &'a mut [bool],
    hit: bool,
    role: Option<RoleScan<'a>>,
}

#[derive(Clone, Copy)]
struct Gap {
    file: Index,
    start: Index,
    end: Index,
    fired: bool,
}

fn spaces(text: &[u8]) -> &[u8] {
    let start = text.iter().take_while(|&&byte| horizontal(byte)).count();
    &text[start..]
}

fn horizontal(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t')
}

fn ident(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn identifier(text: &[u8]) -> &[u8] {
    &text[..text.iter().take_while(|&&byte| ident(byte)).count()]
}

// Comments and strings count as occurrences. Recursion follows a macro's
// name as an identifier, so unrelated substrings do not name the fixture.
fn line_names(text: &[u8], needle: &[u8], whole_word: bool) -> bool {
    !needle.is_empty()
        && text.windows(needle.len()).enumerate().any(|(i, window)| {
            window == needle
                && (!whole_word
                    || ((i == 0 || !ident(text[i - 1]))
                        && text
                            .get(i + needle.len())
                            .is_none_or(|&byte| !ident(byte))))
        })
}

fn define_name(text: &[u8]) -> Checked<Option<&[u8]>> {
    let Some(body) = spaces(text).strip_prefix(b"#") else {
        return Ok(None);
    };
    let Some(rest) = spaces(body).strip_prefix(b"define") else {
        return Ok(None);
    };
    if !rest.first().is_some_and(|&byte| horizontal(byte)) {
        return Ok(None);
    }
    let name = identifier(spaces(rest));
    if name.is_empty() || name.len() >= TOKEN_MAX {
        return Err(());
    }
    Ok(Some(name))
}

// A runner re-executes itself with a TF_ROLE literal as argv[1].
fn role_entry(text: &[u8]) -> Option<Checked<(&[u8], &[u8])>> {
    let text = spaces(text);
    if text.starts_with(b"#") {
        return None;
    }
    let marker = b"TF_ROLE(";
    let at = text.windows(marker.len()).position(|part| part == marker)?;
    Some(parse_role(&text[at + marker.len()..]).ok_or(()))
}

fn parse_role(text: &[u8]) -> Option<(&[u8], &[u8])> {
    let phase_end = text.iter().position(|&byte| byte == b',')?;
    let quoted = spaces(&text[phase_end + 1..]).strip_prefix(b"\"")?;
    let end = quoted
        .iter()
        .position(|&byte| matches!(byte, b'"' | b'\\'))?;
    if quoted[end] != b'"' || end == 0 || end >= TOKEN_MAX {
        return None;
    }
    let text = spaces(spaces(&quoted[end + 1..]).strip_prefix(b",")?);
    let handler = identifier(text);
    if handler.is_empty()
        || handler.len() >= TOKEN_MAX
        || !spaces(&text[handler.len()..]).starts_with(b")")
    {
        return None;
    }
    let literal = quoted[..end].split(|&byte| byte == 0).next()?;
    Some((literal, handler))
}

impl<'a> Reach<'a> {
    fn source_path(&self, file: Index) -> Text {
        unsafe { cbm_ti_source_path(self.input.source, file) }
    }

    pub(super) fn source_find(&self, path: Text) -> Index {
        unsafe { cbm_ti_source_find(self.input.source, path) }
    }

    fn source_lines(&self, file: Index) -> Index {
        unsafe { cbm_ti_source_line_count(self.input.source, file) }
    }

    fn source_line(&self, file: Index, line: Index) -> Option<&'a [u8]> {
        let mut len = 0;
        let text = unsafe {
            cbm_ti_source_line(self.input.source, file, line, &mut len)
        };
        (!text.is_null()).then(|| unsafe { view(text, len) })
    }

    fn scan_literal(
        &self,
        needle: &[u8],
        whole_word: bool,
        depth: usize,
        scan: &mut LiteralScan<'_>,
    ) -> Checked<()> {
        let files = unsafe { cbm_ti_source_file_count(self.input.source) };
        for file in 0..files {
            let path = self.source_path(file);
            let path_bytes = unsafe { bytes(path) };
            let test_file = self.test_file(path_bytes);
            let runner_file =
                scan.role.is_some() && self.runner_file(path_bytes);
            if !test_file && !runner_file {
                continue;
            }
            for line in 1..=self.source_lines(file) {
                let Some(text) = self.source_line(file, line) else {
                    continue;
                };
                if !line_names(text, needle, whole_word) {
                    continue;
                }
                if test_file {
                    match define_name(text) {
                        Ok(Some(name)) if name == needle => continue,
                        Ok(Some(name)) if depth < DEFINE_DEPTH_MAX => {
                            self.scan_literal(name, true, depth + 1, scan)?;
                            continue;
                        }
                        Err(()) | Ok(Some(_)) => {
                            let mut note = Note::<400>::new();
                            note.location(path_bytes, line);
                            note.append(b" defines a macro naming ");
                            note.append(needle);
                            note.append(b" that the scan cannot follow");
                            return self.fallback(
                                Fallback::SelectionUnavailable,
                                note.text(),
                            );
                        }
                        Ok(None) => {}
                    }
                }
                let Ok(definition) = self.enclosing(path, line) else {
                    return self.graph_error();
                };
                let Some(def) = definition else { continue };
                if let Some(role) = &mut scan.role
                    && role.handlers.contains(&def.id)
                {
                    if !role.starters[role.first_starter..].contains(&def.id) {
                        self.allocate(role.starters.push(def.id))?;
                    }
                    continue;
                }
                if !test_file {
                    continue;
                }
                if scan.role.as_ref().is_none_or(|role| {
                    !runner_file || role.sinks.contains(&def.id)
                }) {
                    scan.hit = true;
                }
                // The runner's helpers can seed the walk, but only its test
                // sinks count as known spawners of a role.
                let case = is_function(def.label)
                    .then(|| {
                        let name = unsafe { bytes(def.name) };
                        self.cases.iter().position(|case| unsafe {
                            bytes(case.file) == path_bytes
                                && bytes(case.name) == name
                        })
                    })
                    .flatten();
                if let Some(index) = case {
                    scan.cases[index] = true;
                } else {
                    self.allocate(scan.seeds.push(def.id))?;
                }
            }
        }
        Ok(())
    }

    pub(super) fn fixtures(&self) -> Checked<(Scratch<Id>, Scratch<bool>)> {
        let mut seeds = Scratch::default();
        let mut cases = Scratch::default();
        for _ in self.cases {
            self.allocate(cases.push(false))?;
        }
        let n = self.input.fixture_count as usize;
        let paths = unsafe { view(self.input.fixture_paths, n) };
        let rules = unsafe { view(self.input.fixture_rules, n) };
        for (&path, &rule_index) in paths.iter().zip(rules) {
            let rule = &self.rules[rule_index as usize];
            let mut key = unsafe { bytes(path) };
            if key.len() < FIXTURE_KEY_MAX {
                let mut scan = LiteralScan {
                    seeds: &mut seeds,
                    cases: &mut cases,
                    hit: false,
                    role: None,
                };
                let mut segments =
                    key.iter().filter(|&&byte| byte == b'/').count() + 1;
                let minimum = if rule.key_path {
                    segments
                } else {
                    (rule.key_segment + 1) as usize
                };
                // The first key named by any test decides. Try both spellings
                // at each level, including the path without its first segment.
                while !scan.hit && segments >= minimum {
                    self.scan_literal(key, false, 0, &mut scan)?;
                    if let Some(first) =
                        key.iter().position(|&byte| byte == b'/')
                    {
                        self.scan_literal(
                            &key[first + 1..],
                            false,
                            0,
                            &mut scan,
                        )?;
                    }
                    let Some(last) = key.iter().rposition(|&byte| byte == b'/')
                    else {
                        break;
                    };
                    key = &key[..last];
                    segments -= 1;
                }
                if scan.hit || rule.unreferenced_ignore {
                    continue;
                }
            }
            // A missing or oversized required fixture key forces run-all.
            for i in 0..self.input.match_count as usize {
                let matched = unsafe { &mut *self.input.matches.add(i) };
                if matched.rule_index == rule_index {
                    matched.resolved = false;
                }
            }
        }
        Ok((seeds, cases))
    }

    fn runner_roles(&self, path: Text, sinks: &mut Sinks<'a>) -> Checked<()> {
        let file = self.source_find(path);
        if file < 0 {
            return Ok(());
        }
        for line in 1..=self.source_lines(file) {
            let Some(entry) = self.source_line(file, line).and_then(role_entry)
            else {
                continue;
            };
            let mut note = Note::<400>::new();
            let Ok((literal, handler)) = entry else {
                note.location(unsafe { bytes(path) }, line);
                note.append(b": a role table entry the engine cannot read");
                return self
                    .fallback(Fallback::SelectionUnavailable, note.text());
            };
            let mut name = [0; TOKEN_MAX];
            name[..handler.len()].copy_from_slice(handler);
            mark_written(&name[..=handler.len()]);
            let name = CStr::from_bytes_with_nul(&name[..=handler.len()])
                .map_err(|_| ())?;
            let nodes = self.nodes_by_name(name)?;
            let mut candidates = nodes.iter().filter(|node| {
                let path = unsafe { bytes(node.file_path) };
                !node.file_path.is_null()
                    && (self.test_file(path) || self.runner_file(path))
                    && unsafe { bytes(node.label) } == b"Function"
            });
            let id = match (candidates.next(), candidates.next()) {
                (Some(node), None) => node.id,
                _ => 0,
            };
            if id == 0 {
                note.location(unsafe { bytes(path) }, line);
                note.append(b": the role handler ");
                note.append(handler);
                note.append(b" is no single function of the tests");
                return self
                    .fallback(Fallback::SelectionUnavailable, note.text());
            }
            self.allocate(sinks.roles.push(Role {
                literal,
                id,
                reached: false,
                scanned: false,
                named: false,
                covered: false,
                started_by: None,
                starters: (0, 0),
            }))?;
            self.allocate(sinks.all.push(id))?;
        }
        Ok(())
    }

    pub(super) fn collect_sinks(&self) -> Checked<Sinks<'a>> {
        let mut sinks = Sinks::default();
        for case in self.cases {
            self.named(case.file, case.name, case.start_line, &mut sinks.all)?;
        }
        for suite in self.suites {
            self.named(
                suite.file,
                suite.name,
                suite.start_line,
                &mut sinks.all,
            )?;
        }
        for (i, runner) in self.runners.iter().enumerate() {
            let path = runner.file;
            if path.is_null() {
                continue;
            }
            if i > 0 {
                let previous = self.runners[i - 1].file;
                if !previous.is_null()
                    && unsafe { bytes(previous) == bytes(path) }
                {
                    continue;
                }
            }
            let Ok(defs) = self.definitions(path) else {
                return self.graph_error();
            };
            for def in defs {
                if is_function(def.label)
                    && !def.variant_span
                    && unsafe { bytes(def.name) } == b"main"
                {
                    self.allocate(sinks.all.push(def.id))?;
                    self.allocate(sinks.main.push(def.id))?;
                }
            }
            self.runner_roles(path, &mut sinks)?;
        }
        Ok(sinks)
    }

    fn load_gaps(&self) -> Checked<Scratch<Gap>> {
        let rows =
            StoreRows::read(cbm_store_free_coverage, |out, count| unsafe {
                cbm_store_coverage_get(
                    self.input.store,
                    PROJECT.as_ptr(),
                    out,
                    count,
                )
            });
        let Ok(rows) = rows else {
            self.fallback(
                Fallback::GraphUnavailable,
                c"the graph's parse gaps could not be read".as_ptr(),
            )?;
            return Err(());
        };
        let mut gaps = Scratch::default();
        for row in &*rows {
            if unsafe { bytes(row.kind) } != b"parse_partial"
                || row.rel_path.is_null()
            {
                continue;
            }
            let file = self.source_find(row.rel_path);
            if file >= 0 && !row.detail.is_null() {
                self.parse_ranges(file, row.detail, &mut gaps)?;
            }
        }
        Ok(gaps)
    }

    // Coverage stores "a-b,c-d,e". Keep strtol's prefix parsing and caps.
    fn parse_ranges(
        &self,
        file: Index,
        mut text: Text,
        gaps: &mut Scratch<Gap>,
    ) -> Checked<()> {
        while unsafe { *text } != 0 {
            let mut end = null_mut();
            let start = unsafe { strtol(text, &mut end, 10) };
            if std::ptr::eq(text, end) {
                break;
            }
            let finish = if unsafe { *end } as u8 == b'-' {
                unsafe { strtol(end.add(1), &mut end, 10) }
            } else {
                start
            };
            if start > 0 && finish >= start && finish - start < 100_000 {
                self.allocate(gaps.push(Gap {
                    file,
                    start: start as Index,
                    end: finish as Index,
                    fired: false,
                }))?;
            }
            if unsafe { *end } as u8 != b',' {
                break;
            }
            text = unsafe { end.add(1) };
        }
        Ok(())
    }

    fn gap_names_reached(&self, gap: &Gap, names: &[&[u8]]) -> bool {
        for line in gap.start..=gap.end {
            let Some(mut text) = self.source_line(gap.file, line) else {
                continue;
            };
            while let Some(at) = text
                .iter()
                .position(|&byte| byte.is_ascii_alphabetic() || byte == b'_')
            {
                let word = identifier(&text[at..]);
                if word.len() < TOKEN_MAX && names.binary_search(&word).is_ok() {
                    return true;
                }
                text = &text[at + word.len()..];
            }
        }
        false
    }

    fn gap_seeds(&self, gap: &Gap, seeds: &mut Scratch<Id>) -> Checked<()> {
        let path = self.source_path(gap.file);
        match self.enclosing(path, gap.start) {
            Ok(Some(def)) => return self.allocate(seeds.push(def.id)),
            Err(()) => {
                return self.fallback(
                    Fallback::GraphUnavailable,
                    c"graph read failed".as_ptr(),
                );
            }
            Ok(None) => {}
        }
        let Ok(defs) = self.definitions(path) else {
            return self.fallback(
                Fallback::GraphUnavailable,
                c"graph read failed".as_ptr(),
            );
        };
        for def in defs {
            if !def.variant_span
                && (gap.start..=gap.end).contains(&def.start_line)
            {
                self.allocate(seeds.push(def.id))?;
            }
        }
        Ok(())
    }

    // A parse gap can hide references written on its own lines. From a
    // matching gap, resume at the enclosing definition or definitions that
    // start on its lines. A file-scope gap never seeds the whole file.
    fn parse_gaps(&self, walk: &mut Walk) -> Checked<()> {
        let mut gaps = self.load_gaps()?;
        while !gaps.is_empty() {
            let mut names = Scratch::default();
            for hit in walk.hits() {
                if matches!(
                    unsafe { bytes(hit.label) },
                    b"Function" | b"Method" | b"Variable" | b"Macro"
                ) {
                    let name = unsafe { bytes(hit.qualified_name) }
                        .rsplit(|&byte| byte == b'.')
                        .next();
                    if let Some(name) = name.filter(|name| name.len() >= 3) {
                        self.allocate(names.push(name))?;
                    }
                }
            }
            names.sort_unstable();
            let mut seeds = Scratch::default();
            for gap in &mut *gaps {
                if !gap.fired && self.gap_names_reached(gap, &names) {
                    gap.fired = true;
                    self.gap_seeds(gap, &mut seeds)?;
                }
            }
            drop(names);
            if seeds.is_empty() {
                break;
            }
            walk.run(self, &seeds)?;
        }
        Ok(())
    }

    fn mark_roles(&self, walk: &Walk, roles: &mut [Role<'_>]) -> Checked<bool> {
        let mut grew = false;
        for role in &mut *roles {
            if !role.reached && walk.reached(role.id) {
                role.reached = true;
                grew = true;
            }
        }
        loop {
            let mut chained = false;
            for i in 0..roles.len() {
                if !roles[i].reached {
                    continue;
                }
                let edges =
                    StoreRows::read(cbm_store_free_edges, |out, count| unsafe {
                        cbm_store_find_edges_by_target_type(
                            self.input.store,
                            roles[i].id,
                            c"CALLS".as_ptr(),
                            out,
                            count,
                        )
                    })
                    .or_else(|()| self.graph_error())?;
                for edge in &*edges {
                    for role in &mut *roles {
                        if !role.reached && role.id == edge.source_id {
                            role.reached = true;
                            chained = true;
                        }
                    }
                }
            }
            if !chained {
                return Ok(grew);
            }
            grew = true;
        }
    }

    fn roles_named(
        &self,
        walk: &Walk,
        roles: &mut [Role<'_>],
        starters: &[Id],
    ) -> Checked<()> {
        for i in 0..roles.len() {
            roles[i].covered = roles
                .iter()
                .any(|other| other.id == roles[i].id && other.named);
        }
        loop {
            let mut grew = false;
            for i in 0..roles.len() {
                if !roles[i].covered
                    && roles.iter().any(|other| {
                        other.covered && starts(other.id, &roles[i], starters)
                    })
                {
                    roles[i].covered = true;
                    grew = true;
                }
            }
            if !grew {
                break;
            }
        }
        for (i, role) in roles.iter().enumerate() {
            if !role.reached || role.covered {
                continue;
            }
            let mut from = i;
            for _ in 0..roles.len() {
                let Some(previous) = roles[from].started_by else {
                    break;
                };
                from = previous;
            }
            let mut note = Note::<700>::new();
            note.append(b"the change reaches the test-runner role ");
            note.append(role.literal);
            note.append(b", which no test names");
            if i != from {
                note.append(b" (it starts ");
                note.append(roles[from].literal);
                note.append(b")");
            }
            if note.len + 2 < 700 && walk.hit(roles[from].id).is_some() {
                note.chain(walk, roles[from].id, b": ");
            }
            return self.fallback(Fallback::RunnerReached, note.text());
        }
        Ok(())
    }

    // Role handlers and runner main are excluded from the literal scan.
    // Handlers that start a different role inherit its reach; handlers that
    // call one in-process inherit reach through CALLS.
    fn role_lift(
        &self,
        walk: &mut Walk,
        sinks: &mut Sinks<'a>,
        cases: &mut [bool],
        starters: &mut Scratch<Id>,
    ) -> Checked<()> {
        let mut handlers = Scratch::default();
        for role in &*sinks.roles {
            self.allocate(handlers.push(role.id))?;
        }
        for &main in &*sinks.main {
            self.allocate(handlers.push(main))?;
        }
        loop {
            let mut grew = self.mark_roles(walk, &mut sinks.roles)?;
            for i in 0..sinks.roles.len() {
                let mut role = sinks.roles[i];
                if !role.reached || role.scanned {
                    continue;
                }
                role.scanned = true;
                let first_starter = starters.len();
                let mut seeds = Scratch::default();
                let mut scan = LiteralScan {
                    seeds: &mut seeds,
                    cases,
                    hit: role.named,
                    role: Some(RoleScan {
                        handlers: &handlers,
                        sinks: &sinks.all,
                        starters,
                        first_starter,
                    }),
                };
                self.scan_literal(role.literal, false, 0, &mut scan)?;
                role.named = scan.hit;
                role.starters = (first_starter, starters.len());
                sinks.roles[i] = role;
                for other in &mut *sinks.roles {
                    if !other.reached && starts(other.id, &role, starters) {
                        other.reached = true;
                        other.started_by = Some(i);
                        grew = true;
                    }
                }
                if !seeds.is_empty() {
                    walk.run(self, &seeds)?;
                    grew = true;
                }
            }
            if !grew {
                return self.roles_named(walk, &mut sinks.roles, starters);
            }
        }
    }

    pub(super) fn lift_fixpoint(
        &self,
        walk: &mut Walk,
        sinks: &mut Sinks<'a>,
        cases: &mut [bool],
        starters: &mut Scratch<Id>,
    ) -> Checked<()> {
        // Each lift can expose gaps or handlers for the other lift.
        let mut before = -1;
        while walk.count() != before {
            before = walk.count();
            self.parse_gaps(walk)?;
            self.role_lift(walk, sinks, cases, starters)?;
        }
        Ok(())
    }
}

fn starts(handler: Id, role: &Role<'_>, starters: &[Id]) -> bool {
    handler != role.id
        && starters[role.starters.0..role.starters.1].contains(&handler)
}
