//! Resolve Go call metadata against the current project's function registry.

use std::ffi::CStr;

/// Resolve the original spelling first, then consult the file's import aliases
/// or its own package. A missing package or binding leaves the call unresolved.
fn resolve<'a, T>(
    callee: &CStr,
    package: Option<&[u8]>,
    import: impl FnOnce(&[u8]) -> Option<&'a [u8]>,
    mut lookup: impl FnMut(&CStr) -> Option<T>,
) -> Option<T> {
    if let Some(found) = lookup(callee) {
        return Some(found);
    }

    let callee = callee.to_bytes();
    if let Some(dot) = callee.iter().position(|&byte| byte == b'.') {
        // Match the native resolver's byte limits, including space for NUL.
        if dot == 0 || dot >= 256 {
            return None;
        }
        let package = import(&callee[..dot])?;
        let mut qualified = [0; 1024];
        lookup(qualify(&mut qualified, package, &callee[dot + 1..])?)
    } else {
        let mut qualified = [0; 512];
        lookup(qualify(&mut qualified, package?, callee)?)
    }
}

fn qualify<'a>(
    buffer: &'a mut [u8],
    package: &[u8],
    name: &[u8],
) -> Option<&'a CStr> {
    let len = package.len().checked_add(name.len())?.checked_add(2)?;
    let qualified = buffer.get_mut(..len)?;
    qualified[..package.len()].copy_from_slice(package);
    qualified[package.len()] = b'.';
    qualified[package.len() + 1..len - 1].copy_from_slice(name);
    qualified[len - 1] = 0;
    CStr::from_bytes_with_nul(qualified).ok()
}

#[cfg(any(feature = "native-engine-abi", test))]
#[path = "go_cross_native.rs"]
mod rows;

#[cfg(test)]
#[path = "go_cross_tests.rs"]
mod tests;
