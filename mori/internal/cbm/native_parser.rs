//! Parser entry points from the vendored Tree-sitter runtime.

use std::ffi::c_char;

#[repr(C)]
pub(crate) struct RawParser {
    _private: [u8; 0],
}

#[repr(C)]
pub(crate) struct RawLanguage {
    _private: [u8; 0],
}

#[repr(C)]
pub(crate) struct RawTree {
    _private: [u8; 0],
}

unsafe extern "C" {
    pub(crate) fn ts_parser_new() -> *mut RawParser;
    pub(crate) fn ts_parser_delete(parser: *mut RawParser);
    pub(crate) fn ts_parser_set_language(
        parser: *mut RawParser,
        language: *const RawLanguage,
    ) -> bool;
    pub(crate) fn ts_parser_parse_string(
        parser: *mut RawParser,
        old_tree: *const RawTree,
        source: *const c_char,
        len: u32,
    ) -> *mut RawTree;
    pub(crate) fn ts_tree_delete(tree: *mut RawTree);
}
