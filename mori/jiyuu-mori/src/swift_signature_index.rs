//! Swift signature metadata and ordered overload buckets for one registry.

use std::alloc::{Layout, alloc};
use std::collections::{HashMap, TryReserveError};
use std::ffi::{CStr, c_char};
use std::ptr;

#[derive(Default)]
struct SwiftSignatureIndex {
    // QNs are canonical pointers owned by the C registry's exact map.
    signatures: HashMap<*const c_char, Signature>,
    overloads: HashMap<Vec<u8>, Vec<*const c_char>>,
}

struct Signature {
    defaults: u64,
    count: u8,
    selector: Vec<u8>,
}

#[repr(C)]
struct SignatureView {
    defaults: u64,
    selector: *const c_char,
    count: u8,
}

impl SwiftSignatureIndex {
    fn set(
        &mut self,
        qn: &CStr,
        base_len: usize,
        defaults: u64,
        count: u8,
        selector: Option<&CStr>,
    ) -> Result<(), TryReserveError> {
        let text = match selector {
            Some(selector) => selector.to_bytes_with_nul(),
            None => &qn.to_bytes_with_nul()[base_len..],
        };
        let mut owned = Vec::new();
        owned.try_reserve_exact(text.len())?;
        owned.extend_from_slice(text);

        let key = qn.as_ptr();
        if !self.signatures.contains_key(&key) {
            self.signatures.try_reserve(1)?;
        }
        self.signatures.insert(
            key,
            Signature {
                defaults,
                count,
                selector: owned,
            },
        );
        self.add_overload(&qn.to_bytes()[..base_len], key)
    }

    fn add_overload(
        &mut self,
        base: &[u8],
        qn: *const c_char,
    ) -> Result<(), TryReserveError> {
        if let Some(bucket) = self.overloads.get_mut(base) {
            if !bucket.contains(&qn) {
                bucket.try_reserve(1)?;
                bucket.push(qn);
            }
        } else {
            self.overloads.try_reserve(1)?;
            let mut key = Vec::new();
            key.try_reserve_exact(base.len())?;
            key.extend_from_slice(base);
            let mut bucket = Vec::new();
            bucket.try_reserve_exact(1)?;
            bucket.push(qn);
            self.overloads.insert(key, bucket);
        }
        Ok(())
    }
}

// Construction is serial. Once published, readers may share the index, and
// its borrowed views are valid until the next set or free. The C registry
// drops it before releasing the canonical QNs in the exact map.
#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_rust_swift_signature_set(
    slot: &mut *mut SwiftSignatureIndex,
    canonical_qn: *const c_char,
    base_len: usize,
    defaults: u64,
    count: u8,
    selector: *const c_char,
) {
    let qn = unsafe { CStr::from_ptr(canonical_qn) };
    let selector =
        (!selector.is_null()).then(|| unsafe { CStr::from_ptr(selector) });
    if slot.is_null() {
        let index = unsafe { alloc(Layout::new::<SwiftSignatureIndex>()) }
            .cast::<SwiftSignatureIndex>();
        if index.is_null() {
            return;
        }
        unsafe { index.write(SwiftSignatureIndex::default()) };
        *slot = index;
    }
    // As in the C index, a signature can still be used by name if growing
    // the overload bucket fails. The next set can retry the bucket insertion.
    let _ = unsafe { &mut **slot }.set(qn, base_len, defaults, count, selector);
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_rust_swift_signature_get(
    index: *const SwiftSignatureIndex,
    canonical_qn: *const c_char,
) -> SignatureView {
    match unsafe { index.as_ref() }
        .and_then(|index| index.signatures.get(&canonical_qn))
    {
        Some(signature) => SignatureView {
            defaults: signature.defaults,
            selector: signature.selector.as_ptr().cast(),
            count: signature.count,
        },
        None => SignatureView {
            defaults: 0,
            selector: ptr::null(),
            count: 0,
        },
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_rust_swift_overloads(
    index: *const SwiftSignatureIndex,
    base: *const c_char,
    count: &mut usize,
) -> *const *const c_char {
    let base = unsafe { CStr::from_ptr(base) }.to_bytes();
    let bucket =
        unsafe { index.as_ref() }.and_then(|index| index.overloads.get(base));
    *count = bucket.map_or(0, Vec::len);
    bucket.map_or(ptr::null(), Vec::as_ptr)
}

#[unsafe(no_mangle)]
unsafe extern "C" fn cbm_rust_swift_signature_free(
    index: *mut SwiftSignatureIndex,
) {
    if !index.is_null() {
        unsafe { drop(Box::from_raw(index)) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    #[test]
    fn compatible_work_overloads_keep_first_insertion_and_canonical_qns() {
        // The Service.work and Other.work candidates from test_registry.c.
        let base = c"proj.Service.work";
        let flag = c"proj.Service.work(flag:Bool)";
        let string = c"proj.Service.work(name:String)";
        let int = c"proj.Service.work(name:Int)";
        let other_base = c"proj.Other.work";
        let other = c"proj.Other.work(name:String)";
        let mut index = ptr::null_mut();
        unsafe {
            for (owner, qn) in [
                (base, flag),
                (base, string),
                (base, int),
                (other_base, other),
            ] {
                cbm_rust_swift_signature_set(
                    &mut index,
                    qn.as_ptr(),
                    owner.count_bytes(),
                    0,
                    1,
                    ptr::null(),
                );
            }
            cbm_rust_swift_signature_set(
                &mut index,
                string.as_ptr(),
                base.count_bytes(),
                1,
                1,
                ptr::null(),
            );
            let mut count = 0;
            let items =
                cbm_rust_swift_overloads(index, base.as_ptr(), &mut count);
            assert_eq!(count, 3);
            assert_eq!(
                std::slice::from_raw_parts(items, count),
                [flag.as_ptr(), string.as_ptr(), int.as_ptr()],
            );
            let items =
                cbm_rust_swift_overloads(index, other_base.as_ptr(), &mut count);
            assert_eq!(count, 1);
            assert_eq!(*items, other.as_ptr());
            let updated = cbm_rust_swift_signature_get(index, string.as_ptr());
            assert_eq!(updated.defaults, 1);
            assert_eq!(updated.count, 1);
            assert_eq!(CStr::from_ptr(updated.selector), c"(name:String)");
            cbm_rust_swift_signature_free(index);
        }
    }

    #[test]
    fn selectors_are_owned_and_signature_data_is_generation_local() {
        // The C suite passes NULL for configure and explicit "()" for its
        // manifest-root methods. Incremental seeders may free the JSON input.
        let base = c"proj.Service.configure";
        let qn = c"proj.Service.configure(a:Int,b:Int,c:Int)";
        let mut first = ptr::null_mut();
        let mut next = ptr::null_mut();
        unsafe {
            let selector = CString::new("()").unwrap();
            cbm_rust_swift_signature_set(
                &mut first,
                qn.as_ptr(),
                base.count_bytes(),
                0,
                0,
                selector.as_ptr(),
            );
            drop(selector);
            assert!(
                cbm_rust_swift_signature_get(next, qn.as_ptr())
                    .selector
                    .is_null()
            );
            cbm_rust_swift_signature_set(
                &mut next,
                qn.as_ptr(),
                base.count_bytes(),
                1 << 1,
                3,
                ptr::null(),
            );
            let explicit = cbm_rust_swift_signature_get(first, qn.as_ptr());
            assert_eq!(CStr::from_ptr(explicit.selector), c"()");
            cbm_rust_swift_signature_free(first);

            let fallback = cbm_rust_swift_signature_get(next, qn.as_ptr());
            assert_eq!(fallback.defaults, 1 << 1);
            assert_eq!(fallback.count, 3);
            assert_eq!(
                CStr::from_ptr(fallback.selector),
                c"(a:Int,b:Int,c:Int)"
            );
            let mut count = 1;
            assert!(
                cbm_rust_swift_overloads(
                    next,
                    c"proj.Unknown".as_ptr(),
                    &mut count
                )
                .is_null()
            );
            assert_eq!(count, 0);
            cbm_rust_swift_signature_free(next);
        }
    }
}
