//! Image reference validation and repository identity shared by Docker and Helm.

use std::ffi::{CStr, c_char};

const REFERENCE_BUFFER_CAPACITY: usize = 512;
const MAX_TAG_LEN: usize = 128;
const MIN_DIGEST_LEN: usize = 32;
const UNKNOWN_REGISTRY: &[u8] = b"unknown";

fn is_registry(first: &[u8]) -> bool {
    first.contains(&b'.') || first.contains(&b':') || first == b"localhost"
}

fn last_segment_start(reference: &[u8]) -> usize {
    reference
        .iter()
        .rposition(|&byte| byte == b'/')
        .map_or(0, |slash| slash + 1)
}

/// Validate the stricter form used for values rendered by Helm.
fn valid_reference(mut reference: &[u8]) -> bool {
    if let Some(at) = reference.iter().position(|&byte| byte == b'@') {
        let digest = &reference[at + 1..];
        let Some(colon) = digest.iter().position(|&byte| byte == b':') else {
            return false;
        };
        let algorithm = &digest[..colon];
        let hex = &digest[colon + 1..];
        if algorithm.is_empty()
            || !algorithm.iter().all(|byte| {
                byte.is_ascii_alphanumeric() || b"+.-_".contains(byte)
            })
            || hex.len() < MIN_DIGEST_LEN
            || !hex.iter().all(u8::is_ascii_hexdigit)
        {
            return false;
        }
        reference = &reference[..at];
    }

    let segment = last_segment_start(reference);
    if let Some(colon) =
        reference[segment..].iter().rposition(|&byte| byte == b':')
    {
        let tag_start = segment + colon;
        let tag = &reference[tag_start + 1..];
        if !tag
            .first()
            .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
            || tag.len() > MAX_TAG_LEN
            || !tag.iter().all(|byte| {
                byte.is_ascii_alphanumeric() || b"_.-".contains(byte)
            })
        {
            return false;
        }
        reference = &reference[..tag_start];
    }

    if let Some(slash) = reference.iter().position(|&byte| byte == b'/') {
        let first = &reference[..slash];
        if is_registry(first) {
            if !valid_domain(first) {
                return false;
            }
            reference = &reference[slash + 1..];
        }
    }
    reference.split(|&byte| byte == b'/').all(valid_component)
}

fn valid_domain(domain: &[u8]) -> bool {
    let port_separator = if domain.starts_with(b"[") {
        let Some(close) = domain.iter().position(|&byte| byte == b']') else {
            return false;
        };
        // The C validator treats the first byte after ']' as the separator.
        (close + 1 < domain.len()).then_some(close + 1)
    } else {
        domain.iter().position(|&byte| byte == b':')
    };
    let host = &domain[..port_separator.unwrap_or(domain.len())];
    if host.is_empty()
        || (!host.starts_with(b"[")
            && !host.iter().all(|byte| {
                byte.is_ascii_alphanumeric() || b".-".contains(byte)
            }))
    {
        return false;
    }
    port_separator.is_none_or(|separator| {
        let port = &domain[separator + 1..];
        !port.is_empty() && port.iter().all(u8::is_ascii_digit)
    })
}

fn valid_component(component: &[u8]) -> bool {
    if !component.first().is_some_and(u8::is_ascii_alphanumeric)
        || !component.last().is_some_and(u8::is_ascii_alphanumeric)
    {
        return false;
    }
    component
        .chunk_by(|left, right| left == right)
        .all(|run| match run[0] {
            b'a'..=b'z' | b'0'..=b'9' | b'-' => true,
            b'.' => run.len() == 1,
            b'_' => run.len() <= 2,
            _ => false,
        })
}

struct ImageIdentity<'a> {
    repository_prefix: &'static [u8],
    repository: &'a [u8],
    registry: &'a [u8],
}

/// Normalize the looser form used by graph writers. The byte cap applies
/// before stripping the digest and tag, as in the native Docker pass.
fn image_identity(reference: &[u8]) -> Option<ImageIdentity<'_>> {
    if !reference
        .iter()
        .all(|byte| byte.is_ascii_alphanumeric() || b"._-/:@+".contains(byte))
    {
        return None;
    }
    let reference =
        &reference[..reference.len().min(REFERENCE_BUFFER_CAPACITY - 1)];
    let name = reference.split(|&byte| byte == b'@').next()?;
    let segment = last_segment_start(name);
    let repository = match name[segment..].iter().position(|&byte| byte == b':')
    {
        Some(colon) => &name[..segment + colon],
        None => name,
    };
    if repository.is_empty() {
        return None;
    }

    if let Some(slash) = repository.iter().position(|&byte| byte == b'/') {
        let first = &repository[..slash];
        if matches!(
            first,
            b"docker.io" | b"index.docker.io" | b"registry-1.docker.io"
        ) {
            let rest = &repository[slash + 1..];
            return Some(ImageIdentity {
                repository_prefix: if rest.contains(&b'/') {
                    b"docker.io/"
                } else {
                    b"docker.io/library/"
                },
                repository: rest,
                registry: b"docker.io",
            });
        }
        if is_registry(first) {
            return Some(ImageIdentity {
                repository_prefix: b"",
                repository,
                registry: first,
            });
        }
    }
    Some(ImageIdentity {
        repository_prefix: b"",
        repository,
        registry: UNKNOWN_REGISTRY,
    })
}

/// # Safety
/// A non-null reference must point to a readable, NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cbm_image_ref_valid(reference: *const c_char) -> bool {
    !reference.is_null()
        && valid_reference(unsafe { CStr::from_ptr(reference) }.to_bytes())
}

/// # Safety
/// A non-null reference must point to a readable, NUL-terminated string.
/// Outputs with nonzero capacity must point to disjoint writable buffers of
/// the given capacities, with no overlap with the input.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mori_docker_image_identity(
    reference: *const c_char,
    repository: *mut c_char,
    repository_capacity: usize,
    registry: *mut c_char,
    registry_capacity: usize,
) -> bool {
    if reference.is_null() {
        return false;
    }
    let Some(identity) =
        image_identity(unsafe { CStr::from_ptr(reference) }.to_bytes())
    else {
        return false;
    };
    unsafe {
        write_c_string(
            repository.cast(),
            repository_capacity,
            identity.repository_prefix,
            identity.repository,
        );
        write_c_string(
            registry.cast(),
            registry_capacity,
            b"",
            identity.registry,
        );
    }
    true
}

unsafe fn write_c_string(
    output: *mut u8,
    capacity: usize,
    prefix: &[u8],
    value: &[u8],
) {
    if capacity == 0 {
        return;
    }
    let max_len = capacity - 1;
    let prefix_len = prefix.len().min(max_len);
    let value_len = value.len().min(max_len - prefix_len);
    // C callers can pass uninitialized stack buffers.
    unsafe {
        std::ptr::copy_nonoverlapping(prefix.as_ptr(), output, prefix_len);
        std::ptr::copy_nonoverlapping(
            value.as_ptr(),
            output.add(prefix_len),
            value_len,
        );
        output.add(prefix_len + value_len).write(0);
    }
}
