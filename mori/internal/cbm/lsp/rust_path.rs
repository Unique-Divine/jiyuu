//! Rust name resolution for scoped expression and type paths.

use std::ffi::{CStr, c_char, c_void};
use std::ptr::null;
use std::slice;

type Arena = c_void;
type Context = c_void;

#[repr(C)]
struct Lookups {
    use_alias:
        unsafe extern "C" fn(*mut Context, *const c_char) -> *const c_char,
    declared_module: unsafe extern "C" fn(*mut Context, *const c_char) -> bool,
    registered_type: unsafe extern "C" fn(*mut Context, *const c_char) -> bool,
}

unsafe extern "C" {
    fn cbm_rust_path_arena_alloc(arena: *mut Arena, bytes: usize)
    -> *mut c_void;
    fn cbm_ffi_mark_written(address: *const c_void, size: usize);
}

fn prelude(name: &[u8]) -> Option<&'static CStr> {
    Some(match name {
        b"String" => c"alloc.string.String",
        b"ToString" => c"alloc.string.ToString",
        b"Vec" => c"alloc.vec.Vec",
        b"VecDeque" => c"alloc.collections.VecDeque",
        b"HashMap" => c"alloc.collections.HashMap",
        b"BTreeMap" => c"alloc.collections.BTreeMap",
        b"HashSet" => c"alloc.collections.HashSet",
        b"BTreeSet" => c"alloc.collections.BTreeSet",
        b"Box" => c"alloc.boxed.Box",
        b"Rc" => c"alloc.rc.Rc",
        b"Arc" => c"alloc.sync.Arc",
        b"Option" => c"core.option.Option",
        b"Some" => c"core.option.Option.Some",
        b"None" => c"core.option.Option.None",
        b"Result" => c"core.result.Result",
        b"Ok" => c"core.result.Result.Ok",
        b"Err" => c"core.result.Result.Err",
        b"Iterator" => c"core.iter.Iterator",
        b"IntoIterator" => c"core.iter.IntoIterator",
        b"Future" => c"core.future.Future",
        b"Clone" => c"core.clone.Clone",
        b"Copy" => c"core.marker.Copy",
        b"Send" => c"core.marker.Send",
        b"Sync" => c"core.marker.Sync",
        b"Default" => c"core.default.Default",
        b"PartialEq" => c"core.cmp.PartialEq",
        b"Eq" => c"core.cmp.Eq",
        b"PartialOrd" => c"core.cmp.PartialOrd",
        b"Ord" => c"core.cmp.Ord",
        b"Hash" => c"core.hash.Hash",
        b"Display" => c"core.fmt.Display",
        b"Debug" => c"core.fmt.Debug",
        b"From" => c"core.convert.From",
        b"Into" => c"core.convert.Into",
        b"TryFrom" => c"core.convert.TryFrom",
        b"TryInto" => c"core.convert.TryInto",
        b"AsRef" => c"core.convert.AsRef",
        b"AsMut" => c"core.convert.AsMut",
        b"Borrow" => c"core.borrow.Borrow",
        b"BorrowMut" => c"core.borrow.BorrowMut",
        b"Deref" => c"core.ops.Deref",
        b"DerefMut" => c"core.ops.DerefMut",
        b"Drop" => c"core.ops.Drop",
        b"RefCell" => c"core.cell.RefCell",
        b"Cell" => c"core.cell.Cell",
        b"Mutex" => c"std.sync.Mutex",
        b"RwLock" => c"std.sync.RwLock",
        _ => return None,
    })
}

fn separator(path: &[u8]) -> Option<usize> {
    path.windows(2).position(|part| part == b"::")
}

fn normalized(path: &[u8]) -> impl Iterator<Item = u8> + '_ {
    let mut bytes = path.iter().copied().peekable();
    std::iter::from_fn(move || match bytes.next()? {
        b':' if bytes.next_if_eq(&b':').is_some() => Some(b'.'),
        byte => Some(byte),
    })
}

#[derive(Clone, Copy)]
struct PathArena(*mut Arena);

impl PathArena {
    fn write(
        &self,
        capacity: usize,
        bytes: impl IntoIterator<Item = u8>,
    ) -> *const c_char {
        // SAFETY: The caller lends a live arena for this operation. Its allocation
        // is fresh and lives until C destroys the arena.
        let data =
            unsafe { cbm_rust_path_arena_alloc(self.0, capacity) }.cast::<u8>();
        if data.is_null() {
            return null();
        }
        let mut length = 0;
        for byte in bytes {
            assert!(length + 1 < capacity);
            // SAFETY: Each byte fits before the terminator.
            unsafe { data.add(length).write(byte) };
            length += 1;
        }
        // SAFETY: The bytes written above and the terminator are initialized.
        unsafe {
            data.add(length).write(0);
            cbm_ffi_mark_written(data.cast(), length + 1);
        }
        data.cast()
    }

    fn normalize(self, path: &CStr) -> *const c_char {
        let bytes = path.to_bytes();
        if bytes.is_empty() {
            return path.as_ptr();
        }
        let result = self.write(bytes.len() + 1, normalized(bytes));
        if result.is_null() {
            path.as_ptr()
        } else {
            result
        }
    }

    fn qualify(self, prefix: &[u8], tail: &[u8]) -> *const c_char {
        let Some(capacity) = prefix
            .len()
            .checked_add(tail.len())
            .and_then(|len| len.checked_add(2))
        else {
            return null();
        };
        self.write(
            capacity,
            prefix.iter().copied().chain(*b".").chain(normalized(tail)),
        )
    }

    fn copy(self, path: &[u8]) -> *const c_char {
        self.write(path.len() + 1, path.iter().copied())
    }
}

struct Resolver<'a> {
    arena: PathArena,
    context: *mut Context,
    lookups: &'a Lookups,
    module: Option<&'a [u8]>,
    self_type: Option<&'a CStr>,
}

impl Resolver<'_> {
    fn alias(&self, name: *const c_char) -> Option<&CStr> {
        if name.is_null() {
            return None;
        }
        // SAFETY: The name is NUL-terminated and the callback borrows both the
        // context and the name. It returns storage valid for this resolution.
        unsafe {
            let value = (self.lookups.use_alias)(self.context, name);
            (!value.is_null()).then(|| CStr::from_ptr(value))
        }
    }

    fn registered(&self, qn: *const c_char) -> bool {
        // SAFETY: The callback borrows a live context and valid C string.
        !qn.is_null()
            && unsafe { (self.lookups.registered_type)(self.context, qn) }
    }

    fn expression(&self, path: &CStr) -> *const c_char {
        let raw = path.to_bytes();
        if raw.is_empty() {
            return path.as_ptr();
        }
        if let Some(receiver) = self.self_type {
            if let Some(tail) = raw.strip_prefix(b"Self::") {
                return self.arena.qualify(receiver.to_bytes(), tail);
            }
            if raw == b"Self" {
                return receiver.as_ptr();
            }
        }
        if let Some(module) = self.module {
            if let Some(tail) = raw.strip_prefix(b"crate::") {
                let end = module
                    .iter()
                    .enumerate()
                    .filter(|&(_, &byte)| byte == b'.')
                    .nth(1)
                    .map_or(module.len(), |(index, _)| index);
                return self.arena.qualify(&module[..end], tail);
            }
            if let Some(tail) = raw.strip_prefix(b"super::")
                && let Some(end) = module.iter().rposition(|&byte| byte == b'.')
            {
                return self.arena.qualify(&module[..end], tail);
            }
        }

        if let Some(split) = separator(raw) {
            let head = &raw[..split];
            let tail = &raw[split + 2..];
            let name = self.arena.copy(head);
            if name.is_null() {
                // Without a copy of the head, neither a use alias nor the
                // prelude can match it: keep the dotted spelling.
                return self.arena.normalize(path);
            }
            if let Some(alias) = self.alias(name) {
                let prefix = self.arena.normalize(alias);
                // SAFETY: Normalization returns either a fresh terminated
                // arena string or the original terminated alias on failure.
                let prefix = unsafe { CStr::from_ptr(prefix) };
                return self.arena.qualify(prefix.to_bytes(), tail);
            }
            if let Some(prefix) = prelude(head) {
                return self.arena.qualify(prefix.to_bytes(), tail);
            }
            // Cargo workspace members, declared dependencies and other
            // scoped paths all use the same unqualified dotted spelling.
            return self.arena.normalize(path);
        }

        if let Some(alias) = self.alias(path.as_ptr()) {
            return self.arena.normalize(alias);
        }
        if let Some(qn) = prelude(raw) {
            return qn.as_ptr();
        }
        self.module
            .map_or(path.as_ptr(), |module| self.arena.qualify(module, raw))
    }

    fn relative_type(&self, dotted: &[u8]) -> Option<*const c_char> {
        let module = self.module?;
        let head_end = dotted.iter().position(|&byte| byte == b'.')?;
        if head_end == 0 {
            return None;
        }
        let head = self.arena.copy(&dotted[..head_end]);
        // SAFETY: The callback borrows the live context and the terminated head.
        if head.is_null()
            || !unsafe { (self.lookups.declared_module)(self.context, head) }
        {
            return None;
        }
        let current = self.arena.qualify(module, dotted);
        let parent = module
            .iter()
            .rposition(|&byte| byte == b'.')
            .map_or_else(null, |index| {
                self.arena.qualify(&module[..index], dotted)
            });
        match (self.registered(current), self.registered(parent)) {
            (true, false) => Some(current),
            (false, true) => Some(parent),
            _ => None,
        }
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_rust_path_resolve(
    arena: *mut Arena,
    context: *mut Context,
    lookups: *const Lookups,
    path: *const c_char,
    module: *const c_char,
    self_type: *const c_char,
    type_path: bool,
) -> *const c_char {
    if path.is_null() {
        return null();
    }
    // SAFETY: C lends the context, callback table, and each non-null
    // NUL-terminated spelling for the duration of this call.
    let (path, module, self_type, lookups) = unsafe {
        (
            CStr::from_ptr(path),
            (!module.is_null()).then(|| CStr::from_ptr(module).to_bytes()),
            (!self_type.is_null()).then(|| CStr::from_ptr(self_type)),
            &*lookups,
        )
    };
    let resolver = Resolver {
        arena: PathArena(arena),
        context,
        lookups,
        module,
        self_type,
    };
    let qn = resolver.expression(path);
    if !type_path || qn.is_null() || resolver.registered(qn) || arena.is_null() {
        return qn;
    }
    // SAFETY: Expression routing returns a valid arena, input, or static string.
    let dotted = unsafe { CStr::from_ptr(qn) }.to_bytes();
    if dotted.iter().copied().eq(normalized(path.to_bytes())) {
        return resolver.relative_type(dotted).unwrap_or(qn);
    }
    qn
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_rust_path_normalize(
    arena: *mut Arena,
    path: *const c_char,
) -> *const c_char {
    if path.is_null() {
        return path;
    }
    // SAFETY: C supplies a terminated path and a live arena.
    PathArena(arena).normalize(unsafe { CStr::from_ptr(path) })
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_rust_path_prelude(
    name: *const c_char,
) -> *const c_char {
    if name.is_null() {
        return null();
    }
    // SAFETY: C supplies a terminated name. Prelude return strings are static.
    prelude(unsafe { CStr::from_ptr(name) }.to_bytes())
        .map_or_else(null, CStr::as_ptr)
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_rust_path_leaf(path: *const c_char) -> *const c_char {
    if path.is_null() {
        return null();
    }
    // SAFETY: C supplies a terminated path and the result borrows that path.
    let mut remain = unsafe { CStr::from_ptr(path) }.to_bytes();
    let mut offset = 0;
    while let Some(split) = separator(remain) {
        let end = split + 2;
        remain = &remain[end..];
        offset += end;
    }
    // SAFETY: offset never advances past the terminator.
    unsafe { path.add(offset) }
}

fn skip_type_arguments(bytes: &[u8]) -> usize {
    let mut depth = 1;
    for (index, &byte) in bytes.iter().enumerate() {
        match byte {
            b'<' => depth += 1,
            b'>' => {
                depth -= 1;
                if depth == 0 {
                    return index + 1;
                }
            }
            _ => {}
        }
    }
    bytes.len()
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_rust_path_strip_turbofish(path: *mut c_char) {
    if path.is_null() {
        return;
    }
    // SAFETY: C lends exclusive access to the terminated, mutable buffer.
    let bytes = unsafe {
        let capacity = CStr::from_ptr(path).to_bytes_with_nul().len();
        slice::from_raw_parts_mut(path.cast::<u8>(), capacity)
    };
    let mut read = 0;
    let mut write = 0;
    let end = bytes.len() - 1;
    while read < end {
        if bytes[read..end].starts_with(b"::<") {
            read += 3;
            read += skip_type_arguments(&bytes[read..end]);
        } else {
            bytes[write] = bytes[read];
            write += 1;
            read += 1;
        }
    }
    bytes[write] = 0;
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_rust_path_strip_impl_args(path: *mut c_char) {
    if path.is_null() {
        return;
    }
    // SAFETY: C lends exclusive access to the terminated, mutable buffer.
    let open = unsafe { CStr::from_ptr(path) }
        .to_bytes()
        .iter()
        .position(|&byte| byte == b'<');
    if let Some(open) = open.filter(|&index| index != 0) {
        // SAFETY: The shared view is no longer live and open is inside the buffer.
        unsafe { path.add(open).write(0) };
    }
}
