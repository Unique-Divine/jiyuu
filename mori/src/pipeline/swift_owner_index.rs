//! Build the nominal Swift owner index before call-resolution workers read it.

use std::cell::Cell;
use std::collections::HashMap;
use std::ffi::{CStr, CString, c_char};
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
    _strings: [CString; 4],
}

#[derive(Default)]
struct SwiftOwnerIndex {
    owners: Vec<OwnerEntry>,
    by_qn: HashMap<Vec<u8>, usize>,
    by_path: HashMap<Vec<u8>, Vec<usize>>,
    by_module_path: HashMap<Vec<u8>, usize>,
    return_types: HashMap<Vec<u8>, CString>,
}

fn module_path(module: &[u8], path: &[u8]) -> Vec<u8> {
    let mut key = Vec::with_capacity(module.len() + 1 + path.len());
    key.extend_from_slice(module);
    key.push(b'\n');
    key.extend_from_slice(path);
    key
}

impl Owner {
    fn manifest(&self) -> Option<&[u8]> {
        (!self.manifest.0.is_null()).then(|| self.manifest.bytes())
    }

    fn key(&self) -> Vec<u8> {
        module_path(self.module.bytes(), self.path.bytes())
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

    fn record_module_path(&mut self, id: usize) {
        let owner = self.owner(id);
        let key = owner.key();
        if self
            .by_module_path
            .get(&key)
            .is_none_or(|&prior| owner.qn.bytes() < self.owner(prior).qn.bytes())
        {
            self.by_module_path.insert(key, id);
        }
    }

    fn add(&mut self, entry: OwnerEntry) {
        let id = self.owners.len();
        let owner = &entry.view;
        self.by_qn.insert(owner.qn.bytes().to_vec(), id);
        if owner.declaration {
            self.by_path
                .entry(owner.path.bytes().to_vec())
                .or_default()
                .push(id);
        }
        self.owners.push(entry);
        if self.owner(id).declaration {
            self.record_module_path(id);
        }
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

    fn finish(&mut self) {
        // Resolve every primary against declarations before indexing extensions
        // of external types. The fallback groups those extensions by module.
        let primaries: Vec<_> = self
            .owners
            .iter()
            .map(|entry| self.primary(&entry.view))
            .collect();
        for (id, primary) in primaries.iter().enumerate().rev() {
            if primary.is_none() {
                self.record_module_path(id);
            }
        }
        // Prepending in reverse graph order preserves graph order for callers.
        for (id, primary) in primaries.into_iter().enumerate().rev() {
            let owner = self.owner(id);
            let primary =
                primary.unwrap_or_else(|| self.by_module_path[&owner.key()]);
            let primary = self.owner(primary);
            owner.primary.set(primary);
            owner.next_member.set(primary.members.replace(owner));
        }
    }

    fn view(&self, id: Option<usize>) -> *const Owner {
        id.map_or(ptr::null(), |id| self.owner(id))
    }
}

#[unsafe(no_mangle)]
extern "C" fn cbm_rust_swift_owners_new() -> *mut SwiftOwnerIndex {
    Box::into_raw(Box::new(SwiftOwnerIndex::default()))
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
) {
    // SAFETY: C supplies terminated strings from the graph, JSON and targets.
    let (qn, path, imports, file) = unsafe {
        (
            CStr::from_ptr(qn),
            CStr::from_ptr(path),
            CStr::from_ptr(imports),
            CStr::from_ptr(file),
        )
    };
    let module_bytes = if !target_name.is_null() {
        // SAFETY: A named target has a manifest and both strings outlive the index.
        let (manifest, name) =
            unsafe { (CStr::from_ptr(manifest), CStr::from_ptr(target_name)) };
        module_path(manifest.to_bytes(), name.to_bytes())
    } else {
        file.to_bytes()[..module_len].to_vec()
    };
    let module =
        CString::new(module_bytes).expect("module consists of C string bytes");
    let module_name = if !target_name.is_null() {
        target_name
    } else if module_len == file.to_bytes().len() {
        // An overlapping or unknown target uses the full file as its own module.
        c"".as_ptr()
    } else {
        let leaf = module
            .to_bytes()
            .iter()
            .rposition(|&b| b == b'/')
            .map_or(0, |pos| pos + 1);
        module.as_ptr().wrapping_add(leaf)
    };
    let strings = [qn.to_owned(), path.to_owned(), module, imports.to_owned()];
    let [qn, path, module, imports] = &strings;
    index.add(OwnerEntry {
        view: Box::new(Owner {
            qn: OwnerText(qn.as_ptr()),
            path: OwnerText(path.as_ptr()),
            module: OwnerText(module.as_ptr()),
            module_name: OwnerText(module_name),
            manifest: OwnerText(manifest),
            imports: OwnerText(imports.as_ptr()),
            declaration,
            primary: Cell::new(ptr::null()),
            members: Cell::new(ptr::null()),
            next_member: Cell::new(ptr::null()),
        }),
        _strings: strings,
    });
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_rust_swift_owner_add_return(
    index: &mut SwiftOwnerIndex,
    qn: *const c_char,
    returned: *const c_char,
) {
    // SAFETY: C supplies terminated strings, copied before the call returns.
    let (qn, returned) =
        unsafe { (CStr::from_ptr(qn), CStr::from_ptr(returned)) };
    index
        .return_types
        .insert(qn.to_bytes().to_vec(), returned.to_owned());
}

#[unsafe(no_mangle)]
extern "C" fn cbm_rust_swift_owners_finish(index: &mut SwiftOwnerIndex) {
    index.finish();
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
        .map_or(ptr::null(), |value| value.as_ptr())
}
