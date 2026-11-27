//! Static TypeScript imports from Mori's live Tree-sitter tree.

use std::ffi::c_void;
use std::ptr::{from_ref, null};

use crate::native_tree::RawNode;

type Emit = unsafe extern "C" fn(
    *mut c_void,
    RawNode,
    *const RawNode,
    *const RawNode,
    bool,
) -> bool;
type Extract = unsafe extern "C" fn(RawNode, *mut c_void, Emit) -> bool;

unsafe extern "C" {
    fn cbm_ts_set_static_import_extractor(extract: Extract);
}

struct Statement {
    source: RawNode,
    context: *mut c_void,
    emit: Emit,
    emitted: bool,
}

impl Statement {
    unsafe fn binding(
        &mut self,
        local: Option<RawNode>,
        member: Option<RawNode>,
        is_default: bool,
    ) -> Option<()> {
        // SAFETY: The callback borrows these nodes only until it returns.
        // Every node points into the same tree as the import statement.
        if !unsafe {
            (self.emit)(
                self.context,
                self.source,
                local.as_ref().map_or(null(), from_ref),
                member.as_ref().map_or(null(), from_ref),
                is_default,
            )
        } {
            return None;
        }
        self.emitted = true;
        Some(())
    }

    unsafe fn named(&mut self, imports: RawNode) -> Option<()> {
        // SAFETY: These child nodes borrow the live tree from the caller.
        unsafe {
            for specifier in
                imports.children().filter(|n| n.is(c"import_specifier"))
            {
                let member = specifier
                    .field(c"name")
                    .or_else(|| specifier.children().next());
                if let Some(member) = member {
                    let alias = specifier.field(c"alias");
                    self.binding(alias, Some(member), false)?;
                }
            }
        }
        Some(())
    }

    unsafe fn clause(&mut self, clause: RawNode) -> Option<()> {
        // SAFETY: These child nodes borrow the live tree from the caller.
        unsafe {
            for child in clause.children() {
                if child.is(c"identifier") {
                    self.binding(Some(child), None, true)?;
                } else if child.is(c"namespace_import") {
                    let name =
                        child.field(c"name").or_else(|| child.children().last());
                    if let Some(name) = name {
                        self.binding(Some(name), None, false)?;
                    }
                } else if child.is(c"named_imports") {
                    self.named(child)?;
                }
            }
        }
        Some(())
    }
}

unsafe fn extract(
    node: RawNode,
    context: *mut c_void,
    emit: Emit,
) -> Option<()> {
    // SAFETY: The caller lends a live import_statement node for this call.
    unsafe {
        let source = node.field(c"source").or_else(|| {
            node.children()
                .filter(|child| {
                    child.is(c"string") || child.is(c"string_literal")
                })
                .last()
        })?;
        let mut statement = Statement {
            source,
            context,
            emit,
            emitted: false,
        };
        for child in node.children() {
            if child.is(c"identifier") {
                statement.binding(Some(child), None, false)?;
            } else if child.is(c"import_clause") {
                statement.clause(child)?;
            }
        }
        if !statement.emitted {
            // A side-effect import or an empty named list binds no name.
            statement.binding(None, None, false)?;
        }
    }
    Some(())
}

unsafe extern "C" fn extract_statement(
    node: RawNode,
    context: *mut c_void,
    emit: Emit,
) -> bool {
    // SAFETY: The C caller keeps the tree and output context live throughout extraction.
    unsafe { extract(node, context, emit).is_some() }
}

/// Install the parser callback for static TypeScript and TSX imports.
///
/// # Safety
///
/// Register during process startup, before any extraction threads run.
pub unsafe fn register() {
    // SAFETY: Startup registration precedes all extractor workers.
    unsafe { cbm_ts_set_static_import_extractor(extract_statement) };
}
