//! Build the nominal Swift owner index before call-resolution workers read it.

use std::alloc::{Layout, alloc};
use std::cell::Cell;
use std::collections::HashMap;
use std::ffi::{CStr, c_char};
use std::ptr;

const MODULE_PATH_CAP: usize = 4096; // CBM_SZ_4K in the C lookups

#[repr(transparent)]
struct OwnerText(*const c_char);

impl OwnerText {
    fn bytes(&self) -> &[u8] {
        // SAFETY: Non-null fields borrow terminated strings for the view's lifetime.
        unsafe { CStr::from_ptr(self.0) }.to_bytes()
    }
}

// The strings and links are borrowed by C until the index is freed. Links
// are written only during construction, before the index is published.
#[repr(C)]
struct Owner {
    qn: OwnerText,
    path: OwnerText,
    module: OwnerText,
    module_name: OwnerText,
    manifest: OwnerText,
    imports: OwnerText,
    declaration: bool,
    primary: Cell<*const Owner>,
    members: Cell<*const Owner>,
    next_member: Cell<*const Owner>,
}

struct OwnerEntry {
    view: Box<Owner>,
    // The view's Vec::as_ptr pointers survive entry moves. These buffers are
    // never mutated after the view is created.
    _strings: [Vec<u8>; 4],
}

#[derive(Default)]
struct SwiftOwnerIndex {
    owners: Vec<OwnerEntry>,
    by_qn: HashMap<Vec<u8>, usize>,
    by_path: HashMap<Vec<u8>, Vec<usize>>,
    by_module_path: HashMap<Vec<u8>, usize>,
    return_types: HashMap<Vec<u8>, Vec<u8>>,
}

fn try_box<T>(value: T) -> Option<Box<T>> {
    let layout = Layout::new::<T>();
    if layout.size() == 0 {
        return Some(Box::new(value));
    }
    // SAFETY: The layout has nonzero size.
    let allocation = unsafe { alloc(layout).cast::<T>() };
    if allocation.is_null() {
        return None;
    }
    // SAFETY: The allocation uses the global allocator and the layout of T.
    unsafe {
        allocation.write(value);
        Some(Box::from_raw(allocation))
    }
}

fn copy_bytes(bytes: &[u8], extra: usize) -> Option<Vec<u8>> {
    let mut copy = Vec::new();
    copy.try_reserve_exact(bytes.len().checked_add(extra)?)
        .ok()?;
    copy.extend_from_slice(bytes);
    Some(copy)
}

fn module_path(module: &[u8], path: &[u8], extra: usize) -> Option<Vec<u8>> {
    let mut key = copy_bytes(
        module,
        1_usize.checked_add(path.len())?.checked_add(extra)?,
    )?;
    key.push(b'\n');
    key.extend_from_slice(path);
    Some(key)
}

impl Owner {
    fn manifest(&self) -> Option<&[u8]> {
        (!self.manifest.0.is_null()).then(|| self.manifest.bytes())
    }

    fn key(&self) -> Option<Vec<u8>> {
        module_path(self.module.bytes(), self.path.bytes(), 0)
    }

    fn imports(&self, name: &[u8]) -> bool {
        !name.is_empty()
            && self.imports.bytes().split(|&b| b == b'|').any(|import| {
                import
                    .strip_prefix(name)
                    .is_some_and(|tail| tail.is_empty() || tail[0] == b'.')
            })
    }
}

impl SwiftOwnerIndex {
    fn owner(&self, id: usize) -> &Owner {
        &self.owners[id].view
    }

    fn record_module_path(&mut self, id: usize) -> Option<()> {
        let owner = self.owner(id);
        let key = owner.key()?;
        if self
            .by_module_path
            .get(&key)
            .is_none_or(|&prior| owner.qn.bytes() < self.owner(prior).qn.bytes())
        {
            self.by_module_path.try_reserve(1).ok()?;
            self.by_module_path.insert(key, id);
        }
        Some(())
    }

    fn add(&mut self, entry: OwnerEntry) -> Option<()> {
        let id = self.owners.len();
        let owner = &entry.view;
        let qn = copy_bytes(owner.qn.bytes(), 0)?;
        self.by_qn.try_reserve(1).ok()?;
        self.by_qn.insert(qn, id);
        if owner.declaration {
            let path = copy_bytes(owner.path.bytes(), 0)?;
            self.by_path.try_reserve(1).ok()?;
            let declarations = self.by_path.entry(path).or_default();
            declarations.try_reserve(1).ok()?;
            declarations.push(id);
        }
        self.owners.try_reserve(1).ok()?;
        self.owners.push(entry);
        if self.owner(id).declaration {
            self.record_module_path(id)?;
        }
        Some(())
    }

    fn imported_primary(&self, owner: &Owner) -> Option<usize> {
        let declarations = self.by_path.get(owner.path.bytes())?;
        let manifest = owner.manifest();
        let mut selected = None;
        let mut local = false;
        let mut ambiguous = false;
        // The C index prepended each declaration to its path list.
        for &id in declarations.iter().rev() {
            let decl = self.owner(id);
            if !owner.imports(decl.module_name.bytes()) {
                continue;
            }
            let same_package = manifest.is_some() && manifest == decl.manifest();
            if local && !same_package {
                continue;
            }
            if same_package && !local {
                selected = None;
                local = true;
                ambiguous = false;
            }
            if let Some(prior) = selected {
                let prior = self.owner(prior);
                if prior.module.bytes() != decl.module.bytes() {
                    ambiguous = true;
                    continue;
                }
                if prior.qn.bytes() <= decl.qn.bytes() {
                    continue;
                }
            }
            selected = Some(id);
        }
        if ambiguous { None } else { selected }
    }

    fn primary(&self, owner: &Owner) -> Option<usize> {
        let module = owner.module.bytes();
        let path = owner.path.bytes();
        let length = module.len() + 1 + path.len();
        if length < MODULE_PATH_CAP {
            let mut key = [0; MODULE_PATH_CAP];
            key[..module.len()].copy_from_slice(module);
            key[module.len()] = b'\n';
            key[module.len() + 1..length].copy_from_slice(path);
            if let Some(&id) = self.by_module_path.get(&key[..length]) {
                return Some(id);
            }
        }
        self.imported_primary(owner)
    }

    fn finish(&mut self) -> Option<()> {
        // Resolve every primary against declarations before indexing extensions
        // of external types. The fallback groups those extensions by module.
        let mut primaries = Vec::new();
        primaries.try_reserve_exact(self.owners.len()).ok()?;
        primaries
            .extend(self.owners.iter().map(|entry| self.primary(&entry.view)));
        for (id, primary) in primaries.iter().enumerate().rev() {
            if primary.is_none() {
                self.record_module_path(id)?;
            }
        }
        // Prepending in reverse graph order preserves graph order for callers.
        for (id, primary) in primaries.into_iter().enumerate().rev() {
            let owner = self.owner(id);
            let primary = match primary {
                Some(primary) => primary,
                None => self.by_module_path[&owner.key()?],
            };
            let primary = self.owner(primary);
            owner.primary.set(primary);
            owner.next_member.set(primary.members.replace(owner));
        }
        Some(())
    }

    fn view(&self, id: Option<usize>) -> *const Owner {
        id.map_or(ptr::null(), |id| self.owner(id))
    }
}

#[unsafe(no_mangle)]
extern "C" fn cbm_rust_swift_owners_new() -> *mut SwiftOwnerIndex {
    try_box(SwiftOwnerIndex::default()).map_or(ptr::null_mut(), Box::into_raw)
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_rust_swift_owners_free(index: *mut SwiftOwnerIndex) {
    if !index.is_null() {
        // SAFETY: C owns this handle and releases it once.
        drop(unsafe { Box::from_raw(index) });
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_rust_swift_owner_add(
    index: &mut SwiftOwnerIndex,
    qn: *const c_char,
    path: *const c_char,
    imports: *const c_char,
    declaration: bool,
    file: *const c_char,
    module_len: usize,
    manifest: *const c_char,
    target_name: *const c_char,
) -> bool {
    (|| {
        // SAFETY: C supplies terminated strings from the graph, JSON and targets.
        let (qn, path, imports, file) = unsafe {
            (
                CStr::from_ptr(qn),
                CStr::from_ptr(path),
                CStr::from_ptr(imports),
                CStr::from_ptr(file),
            )
        };
        let mut module = if !target_name.is_null() {
            // SAFETY: A named target has a manifest and both strings outlive the index.
            let (manifest, name) = unsafe {
                (CStr::from_ptr(manifest), CStr::from_ptr(target_name))
            };
            module_path(manifest.to_bytes(), name.to_bytes(), 1)?
        } else {
            copy_bytes(&file.to_bytes()[..module_len], 1)?
        };
        module.push(0);
        let strings = [
            copy_bytes(qn.to_bytes_with_nul(), 0)?,
            copy_bytes(path.to_bytes_with_nul(), 0)?,
            module,
            copy_bytes(imports.to_bytes_with_nul(), 0)?,
        ];
        let [qn, path, module, imports] = &strings;
        let module_name = if !target_name.is_null() {
            target_name
        } else if module_len == file.to_bytes().len() {
            // An overlapping or unknown target uses the full file as its own module.
            c"".as_ptr()
        } else {
            let leaf = module
                .iter()
                .rposition(|&b| b == b'/')
                .map_or(0, |pos| pos + 1);
            module.as_ptr().wrapping_add(leaf).cast()
        };
        index.add(OwnerEntry {
            view: try_box(Owner {
                qn: OwnerText(qn.as_ptr().cast()),
                path: OwnerText(path.as_ptr().cast()),
                module: OwnerText(module.as_ptr().cast()),
                module_name: OwnerText(module_name),
                manifest: OwnerText(manifest),
                imports: OwnerText(imports.as_ptr().cast()),
                declaration,
                primary: Cell::new(ptr::null()),
                members: Cell::new(ptr::null()),
                next_member: Cell::new(ptr::null()),
            })?,
            _strings: strings,
        })
    })()
    .is_some()
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_rust_swift_owner_add_return(
    index: &mut SwiftOwnerIndex,
    qn: *const c_char,
    returned: *const c_char,
) -> bool {
    (|| {
        // SAFETY: C supplies terminated strings, copied before the call returns.
        let (qn, returned) =
            unsafe { (CStr::from_ptr(qn), CStr::from_ptr(returned)) };
        let key = copy_bytes(qn.to_bytes(), 0)?;
        let value = copy_bytes(returned.to_bytes_with_nul(), 0)?;
        index.return_types.try_reserve(1).ok()?;
        index.return_types.insert(key, value);
        Some(())
    })()
    .is_some()
}

#[unsafe(no_mangle)]
extern "C" fn cbm_rust_swift_owners_finish(index: &mut SwiftOwnerIndex) -> bool {
    index.finish().is_some()
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_rust_swift_owner_by_qn(
    index: &SwiftOwnerIndex,
    qn: *const c_char,
) -> *const Owner {
    if qn.is_null() {
        return ptr::null();
    }
    // SAFETY: C supplies a terminated QN or null.
    index.view(
        index
            .by_qn
            .get(unsafe { CStr::from_ptr(qn) }.to_bytes())
            .copied(),
    )
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_rust_swift_owner_by_module_path(
    index: &SwiftOwnerIndex,
    key: *const c_char,
) -> *const Owner {
    // SAFETY: C supplies a terminated module/path key.
    index.view(
        index
            .by_module_path
            .get(unsafe { CStr::from_ptr(key) }.to_bytes())
            .copied(),
    )
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_rust_swift_owner_imports(
    owner: &Owner,
    name: *const c_char,
) -> bool {
    // SAFETY: C supplies a terminated module name.
    owner.imports(unsafe { CStr::from_ptr(name) }.to_bytes())
}

#[unsafe(no_mangle)]
extern "C" fn cbm_rust_swift_owner_primary(
    index: &SwiftOwnerIndex,
    owner: &Owner,
    imports_only: bool,
) -> *const Owner {
    index.view(if imports_only {
        index.imported_primary(owner)
    } else {
        index.primary(owner)
    })
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_rust_swift_owner_return(
    index: &SwiftOwnerIndex,
    qn: *const c_char,
) -> *const c_char {
    // SAFETY: C supplies a terminated QN.
    let qn = unsafe { CStr::from_ptr(qn) };
    index
        .return_types
        .get(qn.to_bytes())
        .map_or(ptr::null(), |value| value.as_ptr().cast())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    #[test]
    fn owner_text_survives_entry_moves() {
        let mut index = SwiftOwnerIndex::default();
        let file = c"Sources/App/Type.swift";
        let cases = [
            (0, None, &b""[..], &b""[..]),
            (7, None, &b"Sources"[..], &b"Sources"[..]),
            (11, None, &b"Sources/App"[..], &b"App"[..]),
            (8, None, &b"Sources/"[..], &b""[..]),
            (file.count_bytes(), None, file.to_bytes(), &b""[..]),
            (
                file.count_bytes(),
                Some(c"App"),
                &b"Package.swift\nApp"[..],
                &b"App"[..],
            ),
        ];
        for id in 0..64 {
            let (module_len, target, _, _) = cases[id % cases.len()];
            let qn = CString::new(format!("proj.Type{id}")).unwrap();
            // SAFETY: Inputs are terminated. The borrowed target data is static.
            assert!(unsafe {
                cbm_rust_swift_owner_add(
                    &mut index,
                    qn.as_ptr(),
                    c"Type".as_ptr(),
                    c"Core.Submodule".as_ptr(),
                    false,
                    file.as_ptr(),
                    module_len,
                    target.map_or(ptr::null(), |_| c"Package.swift".as_ptr()),
                    target.map_or(ptr::null(), CStr::as_ptr),
                )
            });
        }
        index.finish().unwrap();
        for id in 0..index.owners.len() {
            let (_, _, module, leaf) = cases[id % cases.len()];
            let qn = CString::new(format!("proj.Type{id}")).unwrap();
            // SAFETY: The lookup receives a terminated QN.
            let view =
                unsafe { cbm_rust_swift_owner_by_qn(&index, qn.as_ptr()) };
            assert!(!view.is_null());
            // SAFETY: The non-null view is borrowed from the live index.
            let owner = unsafe { &*view };
            assert_eq!(owner.qn.bytes(), qn.to_bytes());
            assert_eq!(owner.path.bytes(), b"Type");
            assert_eq!(owner.module.bytes(), module);
            assert_eq!(owner.module_name.bytes(), leaf);
            assert!(owner.imports(b"Core"));
        }
    }
}
