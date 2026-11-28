//! Build Go type and callable registry rows from local or cross-file definitions.

use std::ffi::{CStr, c_char, c_int, c_void};
use std::ptr;

use crate::go_eval::native::{
    Type, cbm_go_eval_copy_name, cbm_type_builtin, cbm_type_named,
    cbm_type_pointer, cbm_type_slice, cbm_type_unknown,
};
use crate::go_types::{TypeMode, is_builtin};

type Names = *const *const c_char;
type Types = *const Type;

const MAX_FIELDS: usize = 63;

const LOCAL: u32 = 0;

#[repr(C)]
struct Definition {
    qualified: *const c_char,
    name: *const c_char,
    label: *const c_char,
    module: *const c_char,
    receiver_qn: *const c_char,
    receiver_spelling: *const c_char,
    return_types: Names,
    return_text: *const c_char,
    ordered_params: Names,
    ordered_count: c_int,
    legacy_params: Names,
    parameter_names: Names,
    base_classes: Names,
    embedded_names: *const c_char,
    field_defs: *const c_char,
    method_names: *const c_char,
    alias_target: *const c_char,
    interface: bool,
}

#[repr(C)]
struct TypeInput {
    qualified: *const c_char,
    name: *const c_char,
    alias_of: *const c_char,
    embeds: Names,
    embed_count: usize,
    methods: Names,
    method_count: usize,
    interface: bool,
}

#[repr(C)]
struct FunctionInput {
    qualified: *const c_char,
    name: *const c_char,
    receiver: *const c_char,
    parameter_names: Names,
    parameters: Types,
    parameter_count: usize,
    returns: Types,
    return_count: usize,
}

#[repr(C)]
struct Operations {
    read: unsafe extern "C" fn(*mut c_void, usize, *mut Definition) -> bool,
    has_type: unsafe extern "C" fn(*mut c_void, *const c_char) -> bool,
    add_type: unsafe extern "C" fn(*mut c_void, *const TypeInput),
    fields:
        unsafe extern "C" fn(*mut c_void, *const c_char, Names, Types, usize),
    add_function: unsafe extern "C" fn(*mut c_void, *const FunctionInput),
}

unsafe extern "C" {
    fn cbm_label_is_type_like(label: *const c_char) -> bool;

}

struct Build<'a> {
    context: *mut c_void,
    arena: *mut c_void,
    ops: &'a Operations,
    module: &'a CStr,
    mode: u32,
}

unsafe fn text<'a>(value: *const c_char) -> Option<&'a CStr> {
    // SAFETY: The host keeps its optional C string live during registration.
    (!value.is_null()).then(|| unsafe { CStr::from_ptr(value) })
}

fn names<'a>(rows: Names) -> impl Iterator<Item = &'a CStr> {
    let mut cursor = rows;
    std::iter::from_fn(move || {
        if cursor.is_null() {
            return None;
        }
        // SAFETY: The input carrier is a terminated array of live C strings.
        let value = unsafe { text(*cursor)? };
        cursor = unsafe { cursor.add(1) };
        Some(value)
    })
}

fn split_pipe(text: Option<&CStr>) -> impl Iterator<Item = &[u8]> {
    text.into_iter()
        .flat_map(|text| text.to_bytes().split(|&byte| byte == b'|'))
        .filter(|part| !part.is_empty())
}

fn legacy_receiver(raw: &[u8]) -> Option<&[u8]> {
    let offset = raw.iter().position(|&byte| byte != b'(' && byte != b' ')?;
    let tail = &raw[offset..];
    let offset = tail.iter().position(|&byte| byte == b' ' || byte == b'*')?;
    let tail = &tail[offset..];
    let offset = tail.iter().position(|&byte| byte != b' ' && byte != b'*')?;
    let name = tail[offset..]
        .split(|&byte| byte == b')' || byte == b' ')
        .next()?;
    (!name.is_empty()).then_some(name)
}

impl Build<'_> {
    fn name(&self, bytes: &[u8]) -> *const c_char {
        // SAFETY: The host copies all bytes into the arena during this call.
        unsafe { cbm_go_eval_copy_name(self.arena, bytes.as_ptr(), bytes.len()) }
    }

    fn qualify(&self, module: &CStr, name: &[u8]) -> *const c_char {
        let module = module.to_bytes();
        let mut qn = Vec::new();
        if qn
            .try_reserve_exact(
                module.len().saturating_add(name.len()).saturating_add(1),
            )
            .is_err()
        {
            return ptr::null();
        }
        qn.extend_from_slice(module);
        qn.push(b'.');
        qn.extend_from_slice(name);
        self.name(&qn)
    }

    fn type_text(&self, text: &[u8], module: &CStr) -> Type {
        let mut remaining = text;
        loop {
            if let Some(rest) = remaining.strip_prefix(b"*") {
                remaining = rest;
            } else if let Some(rest) = remaining.strip_prefix(b"[]") {
                remaining = rest;
            } else {
                break;
            }
        }
        // SAFETY: All type constructors allocate in the live registry arena.
        unsafe {
            let mut ty = if remaining.is_empty() {
                cbm_type_unknown()
            } else if is_builtin(remaining, TypeMode::Signature) {
                cbm_type_builtin(self.arena, self.name(remaining))
            } else {
                cbm_type_named(self.arena, self.qualify(module, remaining))
            };
            for &byte in text[..text.len() - remaining.len()].iter().rev() {
                match byte {
                    b'*' => ty = cbm_type_pointer(self.arena, ty),
                    b']' => ty = cbm_type_slice(self.arena, ty),
                    b'[' => {}
                    _ => unreachable!(),
                }
            }
            ty
        }
    }

    fn add_type(&self, row: &TypeInput) {
        // SAFETY: The host copies the pointer arrays and retains only arena strings.
        unsafe { (self.ops.add_type)(self.context, row) };
    }

    fn alias_target(
        &self,
        module: &CStr,
        target: Option<&CStr>,
    ) -> *const c_char {
        let Some(target) =
            target.filter(|text| !text.is_empty() && !module.is_empty())
        else {
            return ptr::null();
        };
        let spelling = target.to_bytes();
        if spelling.iter().all(|byte| {
            byte.is_ascii_alphanumeric() || *byte == b'_' || *byte >= 0x80
        }) {
            self.qualify(module, spelling)
        } else {
            ptr::null()
        }
    }

    fn container(&self, def: &Definition, module: &CStr) {
        let (embeds, methods, alias_of) = if self.mode == LOCAL {
            let embeds = names(def.base_classes)
                .map(|name| {
                    let name = name.to_bytes();
                    let start = name
                        .iter()
                        .position(|&byte| byte != b'*')
                        .unwrap_or(name.len());
                    self.qualify(module, &name[start..])
                })
                .collect::<Vec<_>>();
            (embeds, Vec::new(), ptr::null())
        } else {
            let embeds = split_pipe(unsafe { text(def.embedded_names) })
                .map(|part| self.name(part))
                .collect();
            let methods = if def.interface {
                split_pipe(unsafe { text(def.method_names) })
                    .map(|part| self.name(part))
                    .collect()
            } else {
                Vec::new()
            };
            (
                embeds,
                methods,
                self.alias_target(module, unsafe { text(def.alias_target) }),
            )
        };
        self.add_type(&TypeInput {
            qualified: def.qualified,
            name: def.name,
            alias_of,
            embeds: embeds.as_ptr(),
            embed_count: embeds.len(),
            methods: methods.as_ptr(),
            method_count: methods.len(),
            interface: def.interface,
        });

        let Some(fields) = (unsafe { text(def.field_defs) }) else {
            return;
        };
        let mut names = [ptr::null(); MAX_FIELDS];
        let mut types = [ptr::null(); MAX_FIELDS];
        let mut count = 0;
        for field in fields.to_bytes().split(|&byte| byte == b'|') {
            let Some(colon) = field.iter().position(|&byte| byte == b':') else {
                continue;
            };
            names[count] = self.name(&field[..colon]);
            types[count] = self.type_text(&field[colon + 1..], module);
            count += 1;
            if count == MAX_FIELDS {
                break;
            }
        }
        if count > 0 {
            // SAFETY: Both slices remain live while the host copies the arrays.
            unsafe {
                (self.ops.fields)(
                    self.context,
                    def.qualified,
                    names.as_ptr(),
                    types.as_ptr(),
                    count,
                )
            };
        }
    }

    fn receiver(&self, def: &Definition, module: &CStr) -> *const c_char {
        let receiver = if let Some(qn) =
            unsafe { text(def.receiver_qn) }.filter(|qn| !qn.is_empty())
        {
            qn.as_ptr()
        } else if self.mode == LOCAL {
            let Some(name) = unsafe { text(def.receiver_spelling) }
                .and_then(|raw| legacy_receiver(raw.to_bytes()))
            else {
                return ptr::null();
            };
            self.qualify(module, name)
        } else {
            return ptr::null();
        };
        let Some(qn) = (unsafe { text(receiver) }) else {
            return ptr::null();
        };
        // SAFETY: The host registry and receiver string stay live across the read.
        if !unsafe { (self.ops.has_type)(self.context, receiver) } {
            let offset = qn
                .to_bytes()
                .iter()
                .rposition(|&byte| byte == b'.')
                .map_or(0, |i| i + 1);
            self.add_type(&TypeInput {
                qualified: receiver,
                name: unsafe { receiver.add(offset) },
                alias_of: ptr::null(),
                embeds: ptr::null(),
                embed_count: 0,
                methods: ptr::null(),
                method_count: 0,
                interface: false,
            });
        }
        receiver
    }

    fn function(&self, def: &Definition, module: &CStr, method: bool) {
        let ordered = self.mode != LOCAL
            || !def.ordered_params.is_null()
            || def.ordered_count > 0;
        let mut params = Vec::new();
        if ordered {
            for i in 0..def.ordered_count.max(0) as usize {
                let spelling = if def.ordered_params.is_null() {
                    None
                } else {
                    // SAFETY: Present ordered storage contains ordered_count pointers.
                    unsafe { text(*def.ordered_params.add(i)) }
                };
                params.push(
                    spelling
                        .filter(|ty| !ty.is_empty() && *ty != c"?")
                        .map_or_else(
                            || unsafe { cbm_type_unknown() },
                            |ty| self.type_text(ty.to_bytes(), module),
                        ),
                );
            }
        } else {
            params.extend(
                names(def.legacy_params)
                    .map(|ty| self.type_text(ty.to_bytes(), module)),
            );
        }

        let returns: Vec<_> = if self.mode != LOCAL {
            split_pipe(unsafe { text(def.return_text) })
                .map(|ty| self.type_text(ty, module))
                .collect()
        } else if !def.return_types.is_null() {
            names(def.return_types)
                .map(|ty| self.type_text(ty.to_bytes(), module))
                .collect()
        } else {
            unsafe { text(def.return_text) }
                .filter(|ty| !ty.is_empty())
                .map(|ty| self.type_text(ty.to_bytes(), module))
                .into_iter()
                .collect()
        };
        let receiver = if method {
            self.receiver(def, module)
        } else {
            ptr::null()
        };
        let row = FunctionInput {
            qualified: def.qualified,
            name: def.name,
            receiver,
            parameter_names: if ordered {
                ptr::null()
            } else {
                def.parameter_names
            },
            parameters: params.as_ptr(),
            parameter_count: params.len(),
            returns: returns.as_ptr(),
            return_count: returns.len(),
        };
        // SAFETY: The host copies both type lists into its arena.
        unsafe { (self.ops.add_function)(self.context, &row) };
    }

    fn register(&self, count: usize) {
        for index in 0..count {
            let mut def = std::mem::MaybeUninit::uninit();
            // SAFETY: The host fills the row for this valid index on success.
            if !unsafe { (self.ops.read)(self.context, index, def.as_mut_ptr()) }
            {
                continue;
            }
            let def = unsafe { def.assume_init() };
            if def.name.is_null() || def.qualified.is_null() {
                continue;
            }
            let Some(label) = (unsafe { text(def.label) }) else {
                continue;
            };
            let module = unsafe { text(def.module) }.unwrap_or(self.module);
            if unsafe { cbm_label_is_type_like(def.label) } {
                self.container(&def, module);
            } else if label == c"Function" || label == c"Method" {
                self.function(&def, module, label == c"Method");
            }
        }
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn mori_go_register_definitions(
    context: *mut c_void,
    arena: *mut c_void,
    ops: *const Operations,
    count: usize,
    module: *const c_char,
    mode: u32,
) {
    // SAFETY: The host holds the operations, registry and definitions live.
    let build = Build {
        context,
        arena,
        ops: unsafe { &*ops },
        module: unsafe { text(module) }.unwrap_or(c""),
        mode,
    };
    build.register(count);
}
