//! Python method resolution, including C3 and the legacy depth-first order.

use std::ffi::{CStr, c_char, c_int, c_void};
use std::{ptr, slice};

const MAX_ORDER: usize = 64;

fn merge<T: Copy>(
    sequences: &mut [&[T]],
    output: &mut [T],
    equal: impl Fn(T, T) -> bool,
) -> Option<usize> {
    let mut count = 0;
    while sequences.iter().any(|row| !row.is_empty()) {
        // The first head absent from every tail preserves local base order.
        let head = sequences
            .iter()
            .filter_map(|row| row.first())
            .copied()
            .find(|&head| {
                !sequences
                    .iter()
                    .any(|row| row.iter().skip(1).any(|&name| equal(head, name)))
            })?;
        *output.get_mut(count)? = head;
        count += 1;
        for row in sequences.iter_mut() {
            if let Some((&first, tail)) = row.split_first()
                && equal(head, first)
            {
                *row = tail;
            }
        }
    }
    Some(count)
}

type Name = *const c_char;
type Method = *const c_void;

fn same_name(left: Name, right: Name) -> bool {
    // SAFETY: A participating name is a live, NUL-terminated registry name.
    left == right
        || (!left.is_null()
            && !right.is_null()
            && unsafe { CStr::from_ptr(left) == CStr::from_ptr(right) })
}

#[repr(C)]
struct Class {
    bases: *const Name,
    alias: Name,
}

#[derive(Clone, Copy)]
#[repr(C)]
struct Order {
    names: *const Name,
    len: c_int,
}

const NOT_CACHED: c_int = -2;
const NO_ORDER: Order = Order {
    names: ptr::null(),
    len: -1,
};

impl Class {
    fn bases(&self) -> impl Iterator<Item = Name> {
        let mut cursor = self.bases;
        std::iter::from_fn(move || {
            if cursor.is_null() {
                return None;
            }
            // SAFETY: The native registry lends a terminated base list.
            let name = unsafe { *cursor };
            if name.is_null() {
                cursor = ptr::null();
                return None;
            }
            cursor = unsafe { cursor.add(1) };
            Some(name)
        })
    }
}

#[repr(C)]
struct Native {
    class: unsafe extern "C" fn(*mut c_void, Name) -> Class,
    method: unsafe extern "C" fn(*mut c_void, Name, Name) -> Method,
    find_order: unsafe extern "C" fn(*mut c_void, Name) -> Order,
    keep_order: unsafe extern "C" fn(*mut c_void, Name, Order),
    copy_order: unsafe extern "C" fn(*mut c_void, Order) -> Order,
}

struct Registry<'a> {
    context: *mut c_void,
    native: &'a Native,
    depth_limit: usize,
}

impl Registry<'_> {
    fn class(&self, name: Name) -> Class {
        // SAFETY: Both the context and name belong to this traversal.
        unsafe { (self.native.class)(self.context, name) }
    }

    fn method(&self, class: Name, member: Name) -> Option<Method> {
        // SAFETY: These names remain live throughout the native lookup.
        let method =
            unsafe { (self.native.method)(self.context, class, member) };
        (!method.is_null()).then_some(method)
    }

    fn cached_order(&self, name: Name) -> Option<Order> {
        // SAFETY: The host lends the cached order until its arena is released.
        let cached = unsafe { (self.native.find_order)(self.context, name) };
        (cached.len != NOT_CACHED).then_some(cached)
    }

    fn order(&self, name: Name, depth: usize) -> Order {
        if let Some(cached) = self.cached_order(name) {
            return cached;
        }

        let order = self.compute_order(name, depth);
        // A depth limit reached as a base may still admit the same hierarchy
        // on its own. Cache failures only from the top-level query.
        if order.len >= 0 || depth == 0 {
            // SAFETY: A successful order already lives in the host arena.
            unsafe { (self.native.keep_order)(self.context, name, order) };
        }
        order
    }

    fn names<'a>(&'a self, order: &Order) -> Option<&'a [Name]> {
        if !(1..=MAX_ORDER as c_int).contains(&order.len)
            || order.names.is_null()
        {
            return None;
        }
        // SAFETY: The native allocator stores each returned order in this
        // traversal's arena and initialized every pointer in the range.
        Some(unsafe { slice::from_raw_parts(order.names, order.len as usize) })
    }

    fn compute_order(&self, name: Name, depth: usize) -> Order {
        if depth > self.depth_limit {
            return NO_ORDER;
        }
        let class = self.class(name);
        let mut bases = [ptr::null(); MAX_ORDER - 1];
        let mut base_count = 0;
        for base in class.bases() {
            let Some(slot) = bases.get_mut(base_count) else {
                return NO_ORDER;
            };
            *slot = base;
            base_count += 1;
        }
        if !class.alias.is_null() {
            return self.order(class.alias, depth + 1);
        }

        let mut rows: [&[Name]; MAX_ORDER] = [&[]; MAX_ORDER];
        let bases = &bases[..base_count];
        for (row, &base) in rows.iter_mut().zip(bases) {
            let Some(order) = self.names(&self.order(base, depth + 1)) else {
                return NO_ORDER;
            };
            *row = order;
        }
        rows[base_count] = bases;

        let mut linearized = [ptr::null(); MAX_ORDER];
        linearized[0] = name;
        let Some(count) =
            merge(&mut rows[..=base_count], &mut linearized[1..], same_name)
        else {
            return NO_ORDER;
        };
        // SAFETY: The host copies the computed order into its arena before return.
        unsafe {
            (self.native.copy_order)(
                self.context,
                Order {
                    names: linearized.as_ptr(),
                    len: (count + 1) as c_int,
                },
            )
        }
    }

    fn depth_first(
        &self,
        name: Name,
        member: Name,
        depth: usize,
    ) -> Option<Method> {
        if depth > self.depth_limit {
            return None;
        }
        if let Some(method) = self.method(name, member) {
            return Some(method);
        }
        let class = self.class(name);
        if !class.alias.is_null()
            && let Some(method) =
                self.depth_first(class.alias, member, depth + 1)
        {
            return Some(method);
        }
        class
            .bases()
            .find_map(|base| self.depth_first(base, member, depth + 1))
    }

    fn lookup(&self, name: Name, member: Name) -> Option<Method> {
        if let Some(method) = self.method(name, member) {
            return Some(method);
        }
        let Some(order) = self.names(&self.order(name, 0)) else {
            return self.depth_first(name, member, 0);
        };
        // An alias's first element belongs to its target and still needs lookup.
        let skip = usize::from(same_name(order[0], name));
        order[skip..]
            .iter()
            .find_map(|&base| self.method(base, member))
    }

    fn lookup_super(&self, name: Name, member: Name) -> SuperTarget {
        let class = self.class(name);
        if class.bases.is_null() {
            return SuperTarget::NONE;
        }
        if let Some(order) = self.names(&self.order(name, 0))
            && let Some(method) = order[1..]
                .iter()
                .find_map(|&base| self.method(base, member))
        {
            return SuperTarget::method(method);
        }
        for base in class.bases() {
            if let Some(method) = self.lookup(base, member) {
                return SuperTarget::method(method);
            }
            if same_name(member, c"__init__".as_ptr()) {
                return SuperTarget {
                    method: ptr::null(),
                    constructor_base: base,
                };
            }
        }
        SuperTarget::NONE
    }
}

#[repr(C)]
struct SuperTarget {
    method: Method,
    constructor_base: Name,
}

impl SuperTarget {
    const NONE: Self = Self {
        method: ptr::null(),
        constructor_base: ptr::null(),
    };

    fn method(method: Method) -> Self {
        Self {
            method,
            ..Self::NONE
        }
    }
}

#[cfg(feature = "native-engine-abi")]
#[unsafe(no_mangle)]
unsafe extern "C" fn mori_py_lookup_inherited(
    context: *mut c_void,
    native: *const Native,
    name: Name,
    member: Name,
    depth_limit: usize,
) -> Method {
    // SAFETY: The host keeps its borrowed registry and callbacks live.
    let registry = Registry {
        context,
        native: unsafe { &*native },
        depth_limit,
    };
    registry.lookup(name, member).unwrap_or(ptr::null())
}

#[cfg(feature = "native-engine-abi")]
#[unsafe(no_mangle)]
unsafe extern "C" fn mori_py_lookup_super(
    context: *mut c_void,
    native: *const Native,
    name: Name,
    member: Name,
    depth_limit: usize,
) -> SuperTarget {
    // SAFETY: The host keeps its borrowed registry and callbacks live.
    let registry = Registry {
        context,
        native: unsafe { &*native },
        depth_limit,
    };
    registry.lookup_super(name, member)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check_order(mut sequences: [&[&str]; 3], expected: &[&str]) {
        let mut output = [""; MAX_ORDER - 1];
        let count = merge(&mut sequences, &mut output, |a, b| a == b).unwrap();
        assert_eq!(&output[..count], expected);
    }

    #[test]
    fn native_inheritance_cases() {
        // test_py_lsp.c: pylsp_super_call and pylsp_round5_super_init both
        // reach Base from Child. pylsp_multi_inheritance_first_base reaches A
        // and B from C(A, B). The current class precedes this output.
        check_order([&["Base"], &["Base"], &[]], &["Base"]);
        check_order([&["A"], &["B"], &["A", "B"]], &["A", "B"]);
    }

    #[test]
    fn diamond_defers_common_base() {
        // Parked Python's pylsp_c3_diamond_and_closure reaches Right.work
        // before Root.work in both ordinary lookup and super().
        check_order(
            [&["Left", "Root"], &["Right", "Root"], &["Left", "Right"]],
            &["Left", "Right", "Root"],
        );
    }

    #[test]
    fn inconsistent_order_and_capacity() {
        let mut output = [0; MAX_ORDER - 1];
        assert_eq!(
            merge(&mut [&[1, 2], &[2, 1]], &mut output, |a, b| a == b),
            None
        );
        let names: [usize; MAX_ORDER] = std::array::from_fn(|i| i);
        assert_eq!(
            merge(&mut [&names[..MAX_ORDER - 1]], &mut output, |a, b| a == b),
            Some(MAX_ORDER - 1)
        );
        assert_eq!(merge(&mut [&names], &mut output, |a, b| a == b), None);
    }

    #[test]
    fn native_names_compare_bytes_and_keep_selected_pointer() {
        let base = [0xff_u8, 0];
        let same_base = base;
        let first = [base.as_ptr().cast()];
        let second = [c"Left".as_ptr(), same_base.as_ptr().cast()];
        let mut output = [ptr::null(); 2];
        let count = merge(&mut [&first, &second, &[]], &mut output, same_name);
        assert_eq!(count, Some(2));
        assert_eq!(output, [second[0], first[0]]);
    }

    use std::collections::HashMap;

    #[derive(Default)]
    struct Fixture {
        bases: HashMap<&'static CStr, Vec<Name>>,
        aliases: HashMap<&'static CStr, Name>,
        methods: HashMap<(&'static CStr, &'static CStr), Method>,
        orders: HashMap<&'static CStr, Order>,
        storage: Vec<Box<[Name]>>,
    }

    impl Fixture {
        fn bases(&mut self, name: &'static CStr, bases: &[&'static CStr]) {
            self.bases.insert(
                name,
                bases
                    .iter()
                    .map(|base| base.as_ptr())
                    .chain([ptr::null()])
                    .collect(),
            );
        }

        fn define(&mut self, name: &'static CStr, member: &'static CStr) {
            self.methods.insert((name, member), name.as_ptr().cast());
        }

        fn registry(&mut self, depth_limit: usize) -> Registry<'_> {
            Registry {
                context: (&raw mut *self).cast(),
                native: &NATIVE,
                depth_limit,
            }
        }
    }

    unsafe extern "C" fn class(ctx: *mut c_void, name: Name) -> Class {
        let fixture = unsafe { &*ctx.cast::<Fixture>() };
        let name = unsafe { CStr::from_ptr(name) };
        Class {
            bases: fixture.bases.get(name).map_or(ptr::null(), Vec::as_ptr),
            alias: fixture.aliases.get(name).copied().unwrap_or(ptr::null()),
        }
    }

    unsafe extern "C" fn method(
        ctx: *mut c_void,
        name: Name,
        member: Name,
    ) -> Method {
        let fixture = unsafe { &*ctx.cast::<Fixture>() };
        let keys = unsafe { (CStr::from_ptr(name), CStr::from_ptr(member)) };
        fixture.methods.get(&keys).copied().unwrap_or(ptr::null())
    }

    unsafe extern "C" fn find_order(ctx: *mut c_void, name: Name) -> Order {
        let fixture = unsafe { &*ctx.cast::<Fixture>() };
        fixture
            .orders
            .get(unsafe { CStr::from_ptr(name) })
            .copied()
            .unwrap_or(Order {
                names: ptr::null(),
                len: NOT_CACHED,
            })
    }

    unsafe extern "C" fn keep_order(ctx: *mut c_void, name: Name, order: Order) {
        let fixture = unsafe { &mut *ctx.cast::<Fixture>() };
        fixture
            .orders
            .insert(unsafe { CStr::from_ptr(name) }, order);
    }

    unsafe extern "C" fn copy_order(ctx: *mut c_void, order: Order) -> Order {
        let fixture = unsafe { &mut *ctx.cast::<Fixture>() };
        let names =
            unsafe { slice::from_raw_parts(order.names, order.len as usize) };
        let owned: Box<[Name]> = names.into();
        let names = owned.as_ptr();
        fixture.storage.push(owned);
        Order { names, ..order }
    }

    static NATIVE: Native = Native {
        class,
        method,
        find_order,
        keep_order,
        copy_order,
    };

    #[test]
    fn inherited_and_super_dispatch_share_diamond_order() {
        let mut fixture = Fixture::default();
        fixture.bases(c"Child", &[c"Left", c"Right"]);
        fixture.bases(c"Left", &[c"Root"]);
        fixture.bases(c"Right", &[c"Root"]);
        fixture.define(c"Root", c"work");
        fixture.define(c"Right", c"work");
        fixture.aliases.insert(c"Alias", c"Right".as_ptr());
        let registry = fixture.registry(16);
        let method = Some(c"Right".as_ptr().cast());
        assert_eq!(registry.lookup(c"Child".as_ptr(), c"work".as_ptr()), method);
        let target = registry.lookup_super(c"Child".as_ptr(), c"work".as_ptr());
        assert_eq!(Some(target.method), method);
        assert!(target.constructor_base.is_null());
        assert_eq!(registry.lookup(c"Alias".as_ptr(), c"work".as_ptr()), method);
        assert!(registry.cached_order(c"Child".as_ptr()).is_some());
    }

    #[test]
    fn invalid_mro_and_missing_constructor_keep_native_fallbacks() {
        let mut fixture = Fixture::default();
        fixture.bases(c"Child", &[c"First", c"Second"]);
        fixture.bases(c"First", &[c"A", c"B"]);
        fixture.bases(c"Second", &[c"B", c"A"]);
        fixture.define(c"A", c"work");
        fixture.define(c"B", c"work");
        let registry = fixture.registry(16);
        let method = c"A".as_ptr().cast();
        assert_eq!(registry.order(c"Child".as_ptr(), 0).len, NO_ORDER.len);
        assert_eq!(
            registry.lookup(c"Child".as_ptr(), c"work".as_ptr()),
            Some(method)
        );
        let target = registry.lookup_super(c"Child".as_ptr(), c"work".as_ptr());
        assert_eq!(target.method, method);
        let target =
            registry.lookup_super(c"Child".as_ptr(), c"__init__".as_ptr());
        assert!(target.method.is_null());
        assert_eq!(target.constructor_base, c"First".as_ptr());
    }

    #[test]
    fn depth_exhaustion_does_not_cache_base_as_invalid() {
        let mut fixture = Fixture::default();
        fixture.bases(c"Child", &[c"Middle"]);
        fixture.bases(c"Middle", &[c"Base"]);
        fixture.define(c"Base", c"work");
        let registry = fixture.registry(1);
        assert_eq!(registry.order(c"Child".as_ptr(), 0).len, NO_ORDER.len);
        assert!(registry.cached_order(c"Middle".as_ptr()).is_none());
        assert_eq!(registry.order(c"Middle".as_ptr(), 0).len, 2);
    }
}
