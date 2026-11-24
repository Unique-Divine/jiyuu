//! Cargo and module reachability for Rust intra-doc links.
//!
//! The native resolver supplies repository-relative paths and the existing
//! rs1 scope records. File and module IDs refer to its read-only fact tables.
//! Build scratch data is dropped once the resolved topology has been copied
//! into the native index, before imports or parallel link lookups run.

use std::collections::HashMap;
use std::ffi::{CStr, c_char, c_int, c_void};
use std::ptr;

const NONE: i32 = -1;
const CONFLICT: i32 = -2;
const ROOTS_KEPT: usize = 4;
const PATH_BUF: usize = 1024;
const BESIDE: u8 = 1;
const UNDER_STEM: u8 = 2;
const SOURCE: u8 = 1;
const MANIFEST: u8 = 2;

#[repr(C)]
struct FileView {
    path: *const c_char,
    scope: *const c_char,
    kind: u8,
    test_path: bool,
    first_module: i32,
    module_count: i32,
    first_decl: i32,
    decl_count: i32,
}

#[repr(C)]
struct ModuleView {
    name: *const c_char,
    file: i32,
    parent: i32,
    path_attr: bool,
    cfg_test: bool,
}

#[repr(C)]
struct DeclView {
    name: *const c_char,
    path: *const c_char,
    module: i32,
    cond_path: bool,
    cfg_test: bool,
}

#[derive(Clone, Copy)]
#[repr(C)]
struct Reach {
    root_package: i32,
    package: i32,
    roots: [i32; ROOTS_KEPT],
    count: i32,
    overflow: bool,
    in_lib: bool,
}

impl Default for Reach {
    fn default() -> Self {
        Self {
            root_package: NONE,
            package: NONE,
            roots: [0; ROOTS_KEPT],
            count: 0,
            overflow: false,
            in_lib: false,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum DepKind {
    Module = 1,
    External,
    Gap,
}

#[repr(C)]
struct DepView {
    key: *const u8,
    key_len: usize,
    kind: DepKind,
    module: i32,
}

#[repr(C)]
struct Attachment {
    child: i32,
    decl: i32,
}

#[repr(C)]
pub(crate) struct CargoOps {
    file: unsafe extern "C" fn(*const c_void, i32) -> FileView,
    module: unsafe extern "C" fn(*const c_void, i32) -> ModuleView,
    decl: unsafe extern "C" fn(*const c_void, i32) -> DeclView,
    set_reach: unsafe extern "C" fn(*mut c_void, i32, Reach),
    set_product: unsafe extern "C" fn(*mut c_void, i32, bool),
    set_attachments: unsafe extern "C" fn(*mut c_void, *const Attachment, usize),
    set_package: unsafe extern "C" fn(
        *mut c_void,
        i32,
        i32,
        bool,
        i32,
        *const u8,
        usize,
        *const DepView,
        usize,
    ),
}

struct Source<'a> {
    path: &'a [u8],
    test_path: bool,
    modules: std::ops::Range<usize>,
    decls: std::ops::Range<usize>,
}

struct Module<'a> {
    name: Option<&'a [u8]>,
    file: usize,
    parent: Option<usize>,
    path_attr: bool,
    cfg_test: bool,
}

struct Decl<'a> {
    name: &'a [u8],
    path: Option<&'a [u8]>,
    module: usize,
    cond_path: bool,
    cfg_test: bool,
}

#[derive(Clone, Copy)]
struct DepRecord<'a> {
    kind: u8,
    name: &'a [u8],
    attr: &'a [u8],
    value: &'a [u8],
}

#[derive(Default)]
struct Package<'a> {
    dir: &'a [u8],
    name: Option<&'a [u8]>,
    lib_name: Option<&'a [u8]>,
    lib_path: Option<&'a [u8]>,
    build: Option<&'a [u8]>,
    has_package: bool,
    workspace: bool,
    broken: bool,
    no_autobins: bool,
    no_autoexamples: bool,
    no_autotests: bool,
    no_autobenches: bool,
    edition: i32,
    workspace_edition: i32,
    targets: Vec<&'a [u8]>,
    raw_deps: Vec<DepRecord<'a>>,
}

#[derive(Clone, Copy)]
enum Target {
    Lib,
    Bin,
    Example,
    Test,
    Bench,
    Build,
}

impl Package<'_> {
    fn auto_discovers(&self, target: Target) -> bool {
        self.has_package
            && !match target {
                Target::Lib => self.lib_path.is_some(),
                Target::Bin => self.no_autobins,
                Target::Example => self.no_autoexamples,
                Target::Test => self.no_autotests,
                Target::Bench => self.no_autobenches,
                Target::Build => self.build.is_some(),
            }
    }
}

// Match strtol on an edition field, including leading ASCII space and sign.
// Saturation matches the native long width on Unix and Windows.
fn edition(value: &[u8]) -> i32 {
    let start = value
        .iter()
        .position(|&b| !matches!(b, b' ' | b'\t'..=b'\r'))
        .unwrap_or(value.len());
    let value = &value[start..];
    let value = value.strip_prefix(b"+").unwrap_or(value);
    if value.is_empty() || !value.iter().all(u8::is_ascii_digit) {
        return -2;
    }
    let limit = if cfg!(windows) || cfg!(target_pointer_width = "32") {
        i64::from(i32::MAX)
    } else {
        i64::MAX
    };
    let year = value.iter().fold(0_i64, |year, digit| {
        year.saturating_mul(10)
            .saturating_add(i64::from(digit - b'0'))
            .min(limit)
    });
    if year >= 2015 { year as i32 } else { -2 }
}

fn read_manifest<'a>(path: &'a [u8], blob: &'a [u8]) -> Package<'a> {
    let mut package = Package {
        dir: parent(path),
        ..Package::default()
    };
    for line in blob.split(|b| *b == b'\n').skip(2) {
        let limit = match line.first() {
            Some(b'K') => 4,
            Some(b'a') => 3,
            _ => 5,
        };
        let mut fields = line.splitn(limit, |b| *b == b'\t').skip(1);
        match line.first() {
            Some(b'K') => {
                let (Some(table), Some(key), Some(value)) =
                    (fields.next(), fields.next(), fields.next())
                else {
                    continue;
                };
                if table == b"package" {
                    package.has_package = true;
                    match key {
                        b"name" => {
                            package.name = (value != b"?").then_some(value)
                        }
                        b"edition" => package.edition = edition(value),
                        b"edition.workspace" => package.edition = -1,
                        b"build" => package.build = Some(value),
                        b"autobins" => package.no_autobins = value == b"false",
                        b"autoexamples" => {
                            package.no_autoexamples = value == b"false"
                        }
                        b"autotests" => package.no_autotests = value == b"false",
                        b"autobenches" => {
                            package.no_autobenches = value == b"false"
                        }
                        _ => {}
                    }
                } else {
                    match (table, key) {
                        (b"lib", b"name") => package.lib_name = Some(value),
                        (b"lib", b"path") => package.lib_path = Some(value),
                        (b"workspace.package", b"edition") => {
                            package.workspace_edition = edition(value)
                        }
                        _ => {}
                    }
                }
            }
            Some(b'a') => {
                if let (Some(b"path"), Some(path)) =
                    (fields.next(), fields.next())
                {
                    package.targets.push(path);
                }
            }
            Some(b'W') => package.workspace = true,
            Some(b'B') => package.broken = true,
            Some(b'X') => {
                if let (Some(kind), Some(name), Some(attr), Some(value)) =
                    (fields.next(), fields.next(), fields.next(), fields.next())
                    && let Some(&kind) = kind.first()
                {
                    package.raw_deps.push(DepRecord {
                        kind,
                        name,
                        attr,
                        value,
                    });
                }
            }
            _ => {}
        }
    }
    // Keep the lexical record order within each dependency.
    package.raw_deps.sort_by_key(|record| record.name);
    package
}

fn parent(path: &[u8]) -> &[u8] {
    path.iter()
        .rposition(|byte| *byte == b'/')
        .map_or(b"", |slash| &path[..slash])
}

fn ancestors(path: &[u8]) -> impl Iterator<Item = &[u8]> {
    std::iter::successors(Some(path), |dir| {
        (!dir.is_empty()).then(|| parent(dir))
    })
}

fn append_path(out: &mut Vec<u8>, part: &[u8]) -> Option<()> {
    if part.is_empty() {
        return Some(());
    }
    if out.len() + part.len() + 2 >= PATH_BUF {
        return None;
    }
    if !out.is_empty() {
        out.push(b'/');
    }
    out.extend(part);
    Some(())
}

fn join_path(dir: &[u8], path: &[u8]) -> Option<Vec<u8>> {
    if path.starts_with(b"/")
        || dir.len() + usize::from(!dir.is_empty()) + path.len() >= PATH_BUF
    {
        return None;
    }
    let mut out = Vec::new();
    for part in dir.split(|b| *b == b'/').chain(path.split(|b| *b == b'/')) {
        match part {
            b"" | b"." => {}
            b".." => {
                if out.is_empty() {
                    return None;
                }
                out.truncate(parent(&out).len());
            }
            _ => append_path(&mut out, part)?,
        }
    }
    Some(out)
}

fn crate_name(name: &[u8]) -> Vec<u8> {
    name.iter()
        .map(|&b| if b == b'-' { b'_' } else { b })
        .collect()
}

type PathMap<'a> = HashMap<&'a [u8], usize>;

fn package_at(dirs: &PathMap<'_>, dir: &[u8]) -> Option<usize> {
    dirs.get::<[u8]>(if dir.is_empty() { b"." } else { dir })
        .copied()
}

fn auto_targets(path: &[u8]) -> impl Iterator<Item = (&[u8], Target)> {
    let mut tail = path.rsplitn(5, |byte| *byte == b'/');
    let file = tail.next().unwrap_or(b"");
    let dir = tail.next();
    let grandparent = tail.next();
    let great_grandparent = tail.next();
    let main = file == b"main.rs";
    let before = |levels| (0..levels).fold(parent(path), |dir, _| parent(dir));
    let cargo_dir = |dir: Option<&[u8]>| match dir {
        Some(b"examples") => Some(Target::Example),
        Some(b"tests") => Some(Target::Test),
        Some(b"benches") => Some(Target::Bench),
        _ => None,
    };
    [
        (dir == Some(b"src".as_slice()) && file == b"lib.rs")
            .then(|| (before(1), Target::Lib)),
        (dir == Some(b"src".as_slice()) && main)
            .then(|| (before(1), Target::Bin)),
        (dir == Some(b"bin".as_slice())
            && grandparent == Some(b"src".as_slice()))
        .then(|| (before(2), Target::Bin)),
        (main
            && grandparent == Some(b"bin".as_slice())
            && great_grandparent == Some(b"src".as_slice()))
        .then(|| (before(3), Target::Bin)),
        cargo_dir(dir).map(|target| (before(1), target)),
        cargo_dir(grandparent)
            .filter(|_| main)
            .map(|target| (before(2), target)),
        (file == b"build.rs").then(|| (before(0), Target::Build)),
    ]
    .into_iter()
    .flatten()
}

#[derive(Default)]
struct FileState {
    is_root: bool,
    reach: Reach,
    pending_views: u8,
    views: u8,
    queued: bool,
}

fn mark_root(
    state: &mut FileState,
    source: &Source<'_>,
    package: usize,
    lib: bool,
    lib_root: &mut i32,
) {
    if !state.is_root {
        state.is_root = true;
        state.reach.root_package = package as i32;
    } else if state.reach.root_package != package as i32 {
        state.reach.root_package = CONFLICT;
    }
    if lib && *lib_root == NONE {
        *lib_root = source.modules.start as i32;
    }
}

struct Topology<'a> {
    files: Vec<Option<Source<'a>>>,
    modules: Vec<Module<'a>>,
    decls: Vec<Decl<'a>>,
    sources: PathMap<'a>,
}

impl Topology<'_> {
    fn source(&self, file: usize) -> &Source<'_> {
        self.files[file].as_ref().expect("source file ID")
    }

    fn crate_roots(
        &self,
        packages: &[Package<'_>],
        dirs: &PathMap<'_>,
    ) -> (Vec<FileState>, Vec<i32>) {
        let mut states: Vec<_> =
            self.files.iter().map(|_| FileState::default()).collect();
        let mut lib_roots = vec![NONE; packages.len()];
        for (id, package) in packages
            .iter()
            .enumerate()
            .filter(|(_, package)| package.has_package)
        {
            let explicit = package
                .lib_path
                .map(|path| (path, true))
                .into_iter()
                .chain(package.targets.iter().map(|&path| (path, false)))
                .chain(
                    package
                        .build
                        .filter(|&b| b != b"false" && b != b"true")
                        .map(|path| (path, false)),
                );
            for (path, lib) in explicit.filter(|&(path, _)| path != b"?") {
                if let Some(file) = join_path(package.dir, path)
                    .and_then(|path| self.sources.get(path.as_slice()).copied())
                {
                    mark_root(
                        &mut states[file],
                        self.source(file),
                        id,
                        lib,
                        &mut lib_roots[id],
                    );
                }
            }
        }
        for (file, source) in self.files.iter().enumerate() {
            let Some(source) = source else { continue };
            for (dir, target) in
                auto_targets(source.path).filter(|(dir, _)| dir.len() < PATH_BUF)
            {
                if let Some(id) = package_at(dirs, dir)
                    && packages[id].auto_discovers(target)
                {
                    mark_root(
                        &mut states[file],
                        source,
                        id,
                        matches!(target, Target::Lib),
                        &mut lib_roots[id],
                    );
                }
            }
        }
        (states, lib_roots)
    }

    fn decl_dir(&self, decl: &Decl<'_>, view: u8) -> Option<Vec<u8>> {
        let mut names = Vec::new();
        let mut module = &self.modules[decl.module];
        while let Some(parent) = module.parent {
            if module.path_attr || names.len() >= PATH_BUF / 2 {
                return None;
            }
            names.push(module.name?);
            module = &self.modules[parent];
        }
        let path = self.source(module.file).path;
        let base = path.rsplit(|b| *b == b'/').next()?;
        let mut dir = Vec::new();
        append_path(&mut dir, parent(path))?;
        if view == UNDER_STEM && (decl.path.is_none() || !names.is_empty()) {
            let stem =
                base.strip_suffix(b".rs").filter(|stem| !stem.is_empty())?;
            append_path(&mut dir, stem)?;
        }
        for name in names.into_iter().rev() {
            append_path(&mut dir, name)?;
        }
        Some(dir)
    }

    fn candidates(&self, decl: &Decl<'_>, view: u8) -> [Option<(usize, u8)>; 2] {
        if decl.cond_path {
            return [None, None];
        }
        let Some(mut dir) = self.decl_dir(decl, view) else {
            return [None, None];
        };
        if let Some(path) = decl.path {
            let file = join_path(&dir, path)
                .and_then(|path| self.sources.get(path.as_slice()).copied());
            return [file.map(|file| (file, BESIDE)), None];
        }
        if !dir.is_empty() {
            dir.push(b'/');
        }
        dir.extend(decl.name);
        let len = dir.len();
        dir.extend(b".rs");
        let stem = (dir.len() < PATH_BUF)
            .then(|| self.sources.get(dir.as_slice()))
            .flatten();
        dir.truncate(len);
        dir.extend(b"/mod.rs");
        let beside = (dir.len() < PATH_BUF)
            .then(|| self.sources.get(dir.as_slice()))
            .flatten();
        [
            stem.map(|&file| (file, UNDER_STEM)),
            beside.map(|&file| (file, BESIDE)),
        ]
    }

    fn propagate_views(&self, states: &mut [FileState], stack: &mut Vec<usize>) {
        while let Some(file) = stack.pop() {
            let views = std::mem::take(&mut states[file].pending_views);
            for view in [BESIDE, UNDER_STEM]
                .into_iter()
                .filter(|view| views & view != 0)
            {
                for decl in self.source(file).decls.clone() {
                    for (child, implied) in self
                        .candidates(&self.decls[decl], view)
                        .into_iter()
                        .flatten()
                    {
                        push_view(states, stack, child, implied);
                    }
                }
            }
        }
    }

    // Propagate from declared targets first. Files reached by no target or
    // mod.rs can supply either directory view. A declaration attaches only
    // when all those views name a single indexed file.
    fn attachments(&self, states: &mut [FileState]) -> Vec<Option<usize>> {
        let mut stack = Vec::new();
        for (file, source) in self.files.iter().enumerate() {
            if let Some(source) = source
                && (states[file].is_root
                    || source.path.rsplit(|b| *b == b'/').next()
                        == Some(b"mod.rs"))
            {
                push_view(states, &mut stack, file, BESIDE);
            }
        }
        self.propagate_views(states, &mut stack);
        for (file, source) in self.files.iter().enumerate() {
            if source.is_some() && states[file].views == 0 {
                push_view(states, &mut stack, file, BESIDE | UNDER_STEM);
            }
        }
        self.propagate_views(states, &mut stack);
        self.decls
            .iter()
            .map(|decl| {
                let views = states[self.modules[decl.module].file].views;
                let mut candidates = [BESIDE, UNDER_STEM]
                    .into_iter()
                    .filter(|view| views & view != 0)
                    .flat_map(|view| {
                        self.candidates(decl, view).into_iter().flatten()
                    })
                    .map(|(file, _)| file);
                let child = candidates.next()?;
                candidates.all(|file| file == child).then_some(child)
            })
            .collect()
    }

    fn file_reach(
        &self,
        states: &mut [FileState],
        children: &[Option<usize>],
        lib_roots: &[i32],
        packages: &[Package<'_>],
        dirs: &PathMap<'_>,
    ) {
        let mut stack = Vec::new();
        for (file, state) in states
            .iter_mut()
            .enumerate()
            .filter(|(_, state)| state.is_root)
        {
            let root = self.source(file).modules.start as i32;
            let pkg = state.reach.root_package;
            state.reach.roots[0] = root;
            state.reach.count = 1;
            state.reach.package = pkg;
            state.reach.in_lib = pkg >= 0 && lib_roots[pkg as usize] == root;
            state.queued = true;
            stack.push(file);
        }
        while let Some(file) = stack.pop() {
            states[file].queued = false;
            let reach = states[file].reach;
            for child in self
                .source(file)
                .decls
                .clone()
                .filter_map(|id| children[id])
            {
                let state = &mut states[child];
                if inherit(&mut state.reach, reach) && !state.queued {
                    state.queued = true;
                    stack.push(child);
                }
            }
        }
        for (state, source) in states.iter_mut().zip(&self.files) {
            if let Some(source) = source
                && state.reach.count == 0
                && state.reach.package == NONE
            {
                let dir = parent(source.path);
                if dir.len() < PATH_BUF {
                    state.reach.package = ancestors(dir)
                        .filter_map(|dir| package_at(dirs, dir))
                        .find(|&id| packages[id].has_package)
                        .map_or(NONE, |id| id as i32);
                }
            }
        }
    }

    fn product(
        &self,
        states: &[FileState],
        children: &[Option<usize>],
        attachments: &[Attachment],
    ) -> (Vec<bool>, i32) {
        let mut product = vec![false; self.modules.len()];
        let mut stack = Vec::new();
        let mut orphans = 0;
        let mut attached = vec![false; self.modules.len()];
        for attachment in attachments {
            attached[attachment.child as usize] = true;
        }
        let seed =
            |file: usize, product: &mut [bool], stack: &mut Vec<usize>| {
                let source = self.source(file);
                let root = source.modules.start;
                if !product[root] && !source.test_path {
                    product[root] = true;
                    stack.push(file);
                }
            };
        for (file, source) in self.files.iter().enumerate() {
            if let Some(source) = source {
                let orphan =
                    !states[file].is_root && !attached[source.modules.start];
                orphans += i32::from(orphan);
                if states[file].is_root || orphan {
                    seed(file, &mut product, &mut stack);
                }
            }
        }
        while let Some(file) = stack.pop() {
            let source = self.source(file);
            for module in source.modules.clone().skip(1) {
                let info = &self.modules[module];
                product[module] = !info.cfg_test
                    && info.parent.is_some_and(|parent| product[parent]);
            }
            for id in source.decls.clone() {
                let decl = &self.decls[id];
                if !decl.cfg_test
                    && product[decl.module]
                    && let Some(child) = children[id]
                {
                    seed(child, &mut product, &mut stack);
                }
            }
        }
        (product, orphans)
    }
}

fn push_view(
    states: &mut [FileState],
    stack: &mut Vec<usize>,
    file: usize,
    views: u8,
) {
    let state = &mut states[file];
    let new = views & !state.views;
    if new != 0 {
        if state.pending_views == 0 {
            stack.push(file);
        }
        state.pending_views |= new;
        state.views |= new;
    }
}

fn inherit(child: &mut Reach, parent: Reach) -> bool {
    let mut changed = parent.overflow && !child.overflow;
    child.overflow |= parent.overflow;
    for root in parent.roots[..parent.count as usize].iter().copied() {
        if child.roots[..child.count as usize].contains(&root) {
            continue;
        }
        if child.count as usize == ROOTS_KEPT {
            changed |= !child.overflow;
            child.overflow = true;
        } else {
            child.roots[child.count as usize] = root;
            child.count += 1;
            changed = true;
        }
    }
    let package = match (child.package, parent.package) {
        (NONE, id) | (id, NONE) => id,
        (a, b) if a == b => a,
        _ => CONFLICT,
    };
    changed |= package != child.package || (parent.in_lib && !child.in_lib);
    child.package = package;
    child.in_lib |= parent.in_lib;
    changed
}

#[derive(Default)]
struct DepSpec<'a> {
    path: Option<&'a [u8]>,
    renamed: bool,
    workspace: bool,
    conflict: bool,
}

impl<'a> DepSpec<'a> {
    fn add(&mut self, record: DepRecord<'a>) {
        match record.attr {
            b"path" => {
                self.conflict |=
                    self.path.is_some_and(|path| path != record.value);
                self.path = Some(record.value);
            }
            b"package" => self.renamed = true,
            b"workspace" if record.value == b"true" => self.workspace = true,
            _ => {}
        }
    }
}

struct Dependency {
    key: Vec<u8>,
    kind: DepKind,
    module: i32,
}

struct Crates<'a, 'facts> {
    packages: &'a [Package<'facts>],
    dirs: &'a PathMap<'facts>,
    lib_roots: &'a [i32],
    lib_names: &'a [Option<Vec<u8>>],
}

impl<'facts> Crates<'_, 'facts> {
    fn dependency(
        &self,
        name: &[u8],
        package: &Package<'_>,
        mut spec: DepSpec<'facts>,
        workspace: Option<&Package<'facts>>,
    ) -> Dependency {
        let mut dep = Dependency {
            key: crate_name(name),
            kind: DepKind::Gap,
            module: 0,
        };
        let mut dir = package.dir;
        if spec.workspace {
            let Some(workspace) = workspace else {
                return dep;
            };
            let start = workspace
                .raw_deps
                .partition_point(|record| record.name < name);
            let mut inherited = DepSpec::default();
            let mut found = false;
            for record in workspace.raw_deps[start..]
                .iter()
                .take_while(|r| r.name == name)
            {
                if record.kind == b'w' {
                    found = true;
                    inherited.add(*record);
                }
            }
            if !found {
                return dep;
            }
            spec = inherited;
            dir = workspace.dir;
        }
        if spec.conflict {
            return dep;
        }
        let Some(path) = spec.path else {
            dep.kind = DepKind::External;
            return dep;
        };
        if path == b"?" {
            return dep;
        }
        if let Some(target) =
            join_path(dir, path).and_then(|dir| package_at(self.dirs, &dir))
            && self.packages[target].has_package
            && self.lib_roots[target] >= 0
        {
            dep.kind = DepKind::Module;
            dep.module = self.lib_roots[target];
            if !spec.renamed
                && let Some(name) = &self.lib_names[target]
            {
                dep.key.clone_from(name);
            }
        }
        dep
    }

    fn package_deps(
        &self,
        id: usize,
        workspace: Option<usize>,
    ) -> Vec<Dependency> {
        let package = &self.packages[id];
        let mut deps = Vec::new();
        for group in package.raw_deps.chunk_by(|a, b| a.name == b.name) {
            let mut own =
                group.iter().filter(|record| record.kind != b'w').peekable();
            if own.peek().is_none() {
                continue;
            }
            let mut spec = DepSpec::default();
            for record in own {
                spec.add(*record);
            }
            deps.push(self.dependency(
                group[0].name,
                package,
                spec,
                workspace.map(|ws| &self.packages[ws]),
            ));
        }
        deps.sort_unstable_by(|a, b| a.key.cmp(&b.key));
        deps.dedup_by(|next, first| {
            if next.key != first.key {
                return false;
            }
            if next.kind != first.kind || next.module != first.module {
                first.kind = DepKind::Gap;
                first.module = 0;
            }
            true
        });
        deps
    }
}

fn package_edition(
    package: &Package<'_>,
    workspace: Option<&Package<'_>>,
) -> i32 {
    let edition = if package.edition == -1 {
        workspace.map_or(-2, |ws| ws.workspace_edition)
    } else {
        package.edition
    };
    match edition {
        0 if !package.broken => 2015,
        _ => edition.max(0),
    }
}

// SAFETY: All views come from the live resolver build and are consumed during
// the call. Output views are borrowed only for the duration of each callback.
unsafe fn bytes<'a>(text: *const c_char) -> Option<&'a [u8]> {
    if text.is_null() {
        None
    } else {
        Some(unsafe { CStr::from_ptr(text) }.to_bytes())
    }
}

fn ids(first: i32, count: i32) -> std::ops::Range<usize> {
    first as usize..(first + count) as usize
}

/// Build the Cargo graph before the native name lookup tables are populated.
///
/// # Safety
/// The host supplies valid callbacks and fact IDs. Strings returned by its
/// readers remain live until this call returns. The host copies callback
/// output synchronously into its own index.
#[unsafe(no_mangle)]
pub(crate) unsafe extern "C" fn mori_rust_doc_cargo_build(
    host: *mut c_void,
    ops: &CargoOps,
    file_count: c_int,
    module_count: c_int,
    decl_count: c_int,
) -> c_int {
    let mut sources = HashMap::new();
    let mut packages = Vec::new();
    let mut dirs = HashMap::new();
    let files = (0..file_count)
        .map(|id| {
            let view = unsafe { (ops.file)(host, id) };
            let path =
                unsafe { bytes(view.path) }.expect("repository file path");
            // The last file row owns a duplicate repository path.
            sources.remove(path);
            match view.kind {
                SOURCE => {
                    sources.insert(path, id as usize);
                    Some(Source {
                        path,
                        test_path: view.test_path,
                        modules: ids(view.first_module, view.module_count),
                        decls: ids(view.first_decl, view.decl_count),
                    })
                }
                MANIFEST => {
                    let package = read_manifest(
                        path,
                        unsafe { bytes(view.scope) }.expect("manifest scope"),
                    );
                    dirs.insert(
                        if package.dir.is_empty() {
                            b"."
                        } else {
                            package.dir
                        },
                        packages.len(),
                    );
                    packages.push(package);
                    None
                }
                _ => None,
            }
        })
        .collect();
    let modules = (0..module_count)
        .map(|id| {
            let view = unsafe { (ops.module)(host, id) };
            Module {
                name: unsafe { bytes(view.name) },
                file: view.file as usize,
                parent: usize::try_from(view.parent).ok(),
                path_attr: view.path_attr,
                cfg_test: view.cfg_test,
            }
        })
        .collect();
    let decls = (0..decl_count)
        .map(|id| {
            let view = unsafe { (ops.decl)(host, id) };
            Decl {
                name: unsafe { bytes(view.name) }
                    .expect("module declaration name"),
                path: unsafe { bytes(view.path) },
                module: view.module as usize,
                cond_path: view.cond_path,
                cfg_test: view.cfg_test,
            }
        })
        .collect();
    let topology = Topology {
        files,
        modules,
        decls,
        sources,
    };
    let (mut states, lib_roots) = topology.crate_roots(&packages, &dirs);
    let children = topology.attachments(&mut states);
    let mut attachments: Vec<_> = children
        .iter()
        .enumerate()
        .filter_map(|(decl, &file)| {
            file.map(|file| Attachment {
                child: topology.source(file).modules.start as i32,
                decl: decl as i32,
            })
        })
        .collect();
    attachments.sort_unstable_by_key(|a| (a.child, a.decl));
    topology.file_reach(&mut states, &children, &lib_roots, &packages, &dirs);
    let (product, orphans) = topology.product(&states, &children, &attachments);
    unsafe {
        (ops.set_attachments)(host, attachments.as_ptr(), attachments.len())
    };
    for (file, state) in states.into_iter().enumerate() {
        if topology.files[file].is_some() {
            unsafe { (ops.set_reach)(host, file as i32, state.reach) };
        }
    }
    for (module, is_product) in product.into_iter().enumerate() {
        unsafe { (ops.set_product)(host, module as i32, is_product) };
    }

    let lib_names: Vec<_> = packages
        .iter()
        .map(|p| {
            p.lib_name
                .or(p.name)
                .filter(|&name| name != b"?")
                .map(crate_name)
        })
        .collect();
    let crates = Crates {
        packages: &packages,
        dirs: &dirs,
        lib_roots: &lib_roots,
        lib_names: &lib_names,
    };
    for (id, package) in packages.iter().enumerate() {
        let workspace = (package.dir.len() < PATH_BUF)
            .then(|| {
                ancestors(package.dir)
                    .filter_map(|dir| package_at(&dirs, dir))
                    .find(|&id| packages[id].workspace)
            })
            .flatten();
        let edition =
            package_edition(package, workspace.map(|ws| &packages[ws]));
        let deps = crates.package_deps(id, workspace);
        let views: Vec<_> = deps
            .iter()
            .map(|dep| DepView {
                key: dep.key.as_ptr(),
                key_len: dep.key.len(),
                kind: dep.kind,
                module: dep.module,
            })
            .collect();
        let (name, len) = lib_names[id]
            .as_ref()
            .map_or((ptr::null(), 0), |name| (name.as_ptr(), name.len()));
        unsafe {
            (ops.set_package)(
                host,
                id as i32,
                edition,
                package.broken,
                lib_roots[id],
                name,
                len,
                views.as_ptr(),
                views.len(),
            );
        }
    }
    orphans
}
