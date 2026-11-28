//! Schedule Python call resolution in source order and manage nested scopes.

use std::ffi::{CStr, c_char, c_void};
use std::ptr;

use crate::native_tree::{BorrowedNode as Node, RawNode};
use crate::python_calls::Name;

type Context = *mut c_void;
type Type = *const c_void;

#[repr(C)]
struct Frame {
    previous: *mut c_void,
    comprehension: bool,
    entered: bool,
}

#[repr(C)]
struct Native {
    enter_node: unsafe extern "C" fn(Context) -> bool,
    leave_node: unsafe extern "C" fn(Context),
    failed: unsafe extern "C" fn(Context) -> bool,
    text: unsafe extern "C" fn(Context, *const RawNode) -> *const c_char,
    statement: unsafe extern "C" fn(Context, *const RawNode),
    invocation: unsafe extern "C" fn(Context, *const RawNode),
    dunder: unsafe extern "C" fn(Context, *const RawNode, Name, *const RawNode),
    if_statement: unsafe extern "C" fn(Context, *const RawNode),
    class_definition: unsafe extern "C" fn(Context, *const RawNode),
    in_class: unsafe extern "C" fn(Context) -> bool,
    enter_scope: unsafe extern "C" fn(Context, bool) -> Frame,
    leave_scope: unsafe extern "C" fn(Context, Frame),
    annotation: unsafe extern "C" fn(Context, Name) -> Type,
    scope_value: unsafe extern "C" fn(Context, Name) -> Type,
    evaluate: unsafe extern "C" fn(Context, *const RawNode) -> Type,
    iterable_element: unsafe extern "C" fn(Context, Type) -> Type,
    bind: unsafe extern "C" fn(Context, Name, Type),
    bind_list: unsafe extern "C" fn(Context, Name, Type),
    bind_for_target: unsafe extern "C" fn(Context, *const RawNode, Type),
}

struct Walk<'a> {
    context: Context,
    native: &'a Native,
}

fn first_named(node: Node<'_>) -> Option<Node<'_>> {
    node.named_children().next()
}

fn dunder(op: &CStr) -> Option<&'static CStr> {
    Some(match op.to_bytes() {
        b"+" => c"__add__",
        b"-" => c"__sub__",
        b"*" => c"__mul__",
        b"/" => c"__truediv__",
        b"//" => c"__floordiv__",
        b"%" => c"__mod__",
        b"**" => c"__pow__",
        b"<<" => c"__lshift__",
        b">>" => c"__rshift__",
        b"&" => c"__and__",
        b"|" => c"__or__",
        b"^" => c"__xor__",
        b"@" => c"__matmul__",
        _ => return None,
    })
}

impl Walk<'_> {
    fn run(&self, node: Node<'_>) {
        // SAFETY: Both operations update this traversal's depth counter.
        if unsafe { (self.native.enter_node)(self.context) } {
            self.visit(node);
            unsafe { (self.native.leave_node)(self.context) };
        }
    }

    fn failed(&self) -> bool {
        // SAFETY: The status lives in the current resolver context.
        unsafe { (self.native.failed)(self.context) }
    }

    fn text(&self, node: Node<'_>) -> Option<&CStr> {
        // SAFETY: The node and its arena-backed spelling remain live here.
        let value = unsafe { (self.native.text)(self.context, node.as_raw()) };
        (!value.is_null()).then(|| unsafe { CStr::from_ptr(value) })
    }

    fn with_scope(&self, class_comprehension: bool, visit: impl FnOnce()) {
        // SAFETY: Each successful entry is paired with a restore of its own
        // frame, including a traversal that records a failure.
        let frame = unsafe {
            (self.native.enter_scope)(self.context, class_comprehension)
        };
        if frame.entered {
            visit();
            unsafe { (self.native.leave_scope)(self.context, frame) };
        }
    }

    fn visit(&self, node: Node<'_>) {
        // SAFETY: This is a live node reached during the native scope borrow.
        unsafe { (self.native.statement)(self.context, node.as_raw()) };
        match node.kind().to_bytes() {
            b"call" => unsafe {
                (self.native.invocation)(self.context, node.as_raw())
            },
            b"binary_operator" => {
                if let (Some(left), Some(operator)) =
                    (node.field(c"left"), node.field(c"operator"))
                    && let Some(name) = self.text(operator).and_then(dunder)
                {
                    self.emit_dunder(left, name, node);
                }
            }
            b"subscript" => {
                if let Some(value) = node.field(c"value") {
                    self.emit_dunder(value, c"__getitem__", node);
                }
            }
            b"if_statement" => {
                unsafe {
                    (self.native.if_statement)(self.context, node.as_raw())
                };
                return;
            }
            b"match_statement" => {
                self.match_statement(node);
                return;
            }
            b"list_comprehension"
            | b"dictionary_comprehension"
            | b"set_comprehension"
            | b"generator_expression" => {
                if unsafe { (self.native.in_class)(self.context) } {
                    self.class_comprehension(node);
                } else {
                    self.comprehension(node);
                }
                return;
            }
            b"function_definition" | b"lambda" => return,
            b"class_definition" => {
                unsafe {
                    (self.native.class_definition)(self.context, node.as_raw())
                };
                return;
            }
            _ => {}
        }

        for child in node.named_children() {
            if self.failed() {
                break;
            }
            self.run(child);
        }
    }

    fn emit_dunder(&self, receiver: Node<'_>, name: &CStr, site: Node<'_>) {
        // SAFETY: Both nodes belong to the active tree.
        unsafe {
            (self.native.dunder)(
                self.context,
                receiver.as_raw(),
                name.into(),
                site.as_raw(),
            )
        };
    }

    fn match_statement(&self, node: Node<'_>) {
        let subject = node.field(c"subject");
        let name = subject
            .filter(|subject| subject.kind() == c"identifier")
            .and_then(|subject| self.text(subject));
        if let Some(subject) = subject {
            self.run(subject);
        }
        if let Some(body) = node.field(c"body") {
            for case in body
                .named_children()
                .filter(|node| node.kind() == c"case_clause")
            {
                self.with_scope(false, || self.case(case, name));
            }
        }
    }

    fn case(&self, clause: Node<'_>, subject: Option<&CStr>) {
        let mut pattern = None;
        let mut block = None;
        for child in clause.named_children() {
            if child.kind() == c"block" {
                block = Some(child);
                break;
            }
            pattern.get_or_insert(child);
        }
        if let (Some(subject), Some(mut pattern)) = (subject, pattern) {
            for _ in 0..3 {
                if !matches!(
                    pattern.kind().to_bytes(),
                    b"case_pattern" | b"pattern" | b"_simple_pattern"
                ) {
                    break;
                }
                let Some(inner) = first_named(pattern) else {
                    break;
                };
                pattern = inner;
            }
            match pattern.kind().to_bytes() {
                b"class_pattern" => self.class_pattern(subject, pattern),
                b"list_pattern" | b"tuple_pattern" => {
                    self.sequence_pattern(subject, pattern)
                }
                _ => {}
            }
        }
        if let Some(block) = block {
            self.run(block);
        }
    }

    fn class_pattern(&self, subject: &CStr, pattern: Node<'_>) {
        let mut children = pattern.named_children();
        if let Some(name) = children.next().and_then(|node| self.text(node)) {
            // SAFETY: Annotation accesses this lexical frame and copies its type.
            let narrowed =
                unsafe { (self.native.annotation)(self.context, name.into()) };
            self.bind(subject, narrowed);
        }
        for child in children.filter(|child| child.kind() == c"capture_pattern")
        {
            if let Some(name) =
                first_named(child).and_then(|node| self.text(node))
            {
                self.bind(name, ptr::null());
            }
        }
    }

    fn sequence_pattern(&self, subject: &CStr, pattern: Node<'_>) {
        // SAFETY: The borrowed name and scope last through this lookup.
        let value =
            unsafe { (self.native.scope_value)(self.context, subject.into()) };
        let element = self.iterable_element(value);
        for child in pattern.named_children() {
            let child = if child.kind() == c"case_pattern" {
                first_named(child).unwrap_or(child)
            } else {
                child
            };
            let capture = match child.kind().to_bytes() {
                b"dotted_name" => {
                    first_named(child).filter(|id| id.kind() == c"identifier")
                }
                b"identifier" => Some(child),
                b"splat_pattern" | b"list_splat_pattern" | b"star_pattern" => {
                    let name = child
                        .named_children()
                        .find_map(|part| match part.kind().to_bytes() {
                            b"identifier" => Some(part),
                            b"dotted_name" => first_named(part),
                            _ => None,
                        })
                        .and_then(|part| self.text(part));
                    if let Some(name) = name
                        && !element.is_null()
                    {
                        // SAFETY: This binding retains both types in the same arena.
                        unsafe {
                            (self.native.bind_list)(
                                self.context,
                                name.into(),
                                element,
                            )
                        };
                    }
                    continue;
                }
                _ => None,
            };
            if let Some(name) = capture.and_then(|node| self.text(node))
                && !element.is_null()
            {
                self.bind(name, element);
            }
        }
    }

    fn bind(&self, name: &CStr, value: Type) {
        // SAFETY: The name and type outlive this scope operation. A null type
        // requests the unknown type for a captured name.
        unsafe { (self.native.bind)(self.context, name.into(), value) };
    }

    fn evaluate(&self, node: Option<Node<'_>>) -> Type {
        // SAFETY: The node is live and native treats absent nodes as unknown.
        unsafe {
            (self.native.evaluate)(
                self.context,
                node.as_ref().map_or(ptr::null(), |node| node.as_raw()),
            )
        }
    }

    fn iterable_element(&self, iter: Type) -> Type {
        // SAFETY: The type was produced by this traversal's evaluation arena.
        unsafe { (self.native.iterable_element)(self.context, iter) }
    }

    fn bind_for(&self, node: Node<'_>, element: Type) {
        let left = node.field(c"left");
        // SAFETY: The binding callback treats absence as a no-op. The type
        // belongs to the active arena, or null requests the unknown type.
        unsafe {
            (self.native.bind_for_target)(
                self.context,
                left.as_ref().map_or(ptr::null(), |node| node.as_raw()),
                element,
            )
        };
    }

    fn comprehension(&self, node: Node<'_>) {
        self.with_scope(false, || {
            // Bind clauses before the body or filters are evaluated.
            for clause in node
                .named_children()
                .filter(|node| node.kind() == c"for_in_clause")
            {
                let element =
                    clause.field(c"right").map_or(ptr::null(), |right| {
                        self.iterable_element(self.evaluate(Some(right)))
                    });
                self.bind_for(clause, element);
            }
            for child in node.named_children() {
                self.run(child);
            }
        });
    }

    fn class_comprehension(&self, node: Node<'_>) {
        if self.failed() {
            return;
        }
        let Some((first_index, first_clause)) = node
            .named_children()
            .enumerate()
            .find(|(_, node)| node.kind() == c"for_in_clause")
        else {
            return;
        };
        let first_right = first_clause.field(c"right");
        let first_type = self.evaluate(first_right);
        if let Some(right) = first_right {
            self.run(right);
        }
        if self.failed() {
            return;
        }
        let first_element = self.iterable_element(first_type);
        self.with_scope(true, || {
            // Evaluate the first iterable in the class frame and all later
            // clauses in the comprehension frame, before entering its body.
            for (index, child) in
                node.named_children().enumerate().skip(first_index)
            {
                if self.failed() {
                    break;
                }
                if child.kind() == c"for_in_clause" {
                    let element = if index == first_index {
                        first_element
                    } else {
                        let right = child.field(c"right");
                        let value = self.evaluate(right);
                        if let Some(right) = right {
                            self.run(right);
                        }
                        if self.failed() {
                            break;
                        }
                        self.iterable_element(value)
                    };
                    self.bind_for(child, element);
                } else {
                    self.run(child);
                }
            }
            for child in node.named_children().take(first_index) {
                if self.failed() {
                    break;
                }
                self.run(child);
            }
        });
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn mori_py_walk(
    context: Context,
    native: *const Native,
    node: *const RawNode,
) {
    // SAFETY: The host lends the resolver context and the tree until return.
    let (native, node) = unsafe { (&*native, Node::from_root(&*node)) };
    Walk { context, native }.run(node);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mirrors_python_operator_and_division_targets() {
        assert_eq!(dunder(c"+"), Some(c"__add__"));
        assert_eq!(dunder(c"/"), Some(c"__truediv__"));
        assert_eq!(dunder(c"//"), Some(c"__floordiv__"));
        assert_eq!(dunder(c"**"), Some(c"__pow__"));
        assert_eq!(dunder(c"@"), Some(c"__matmul__"));
        assert_eq!(dunder(c" and "), None);
        assert_eq!(dunder(c"+="), None);
    }
}
