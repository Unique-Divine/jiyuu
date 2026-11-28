//! Regression tests for the language-spec tables.
//!
//! `lang_specs.golden` is the dump of every row as the C tables served it
//! when the Rust tables replaced them (the parity suite showed the two equal
//! on every field). A deliberate table change updates the golden file: run
//! the tests with `MORI_BLESS_LANG_SPECS=1` and review the diff.

use std::ffi::{CStr, c_char, c_int};
use std::fmt::Write as _;

use super::*;

unsafe extern "C" {
    fn ts_language_symbol_count(language: *const RawLanguage) -> u32;
}

fn kinds_text(list: *const *const c_char) -> String {
    if list.is_null() {
        return "NULL".to_owned();
    }
    let mut out = String::from("[");
    let mut i = 0;
    loop {
        // SAFETY: Every kind list in the tables ends in a null pointer.
        let name = unsafe { *list.add(i) };
        if name.is_null() {
            break;
        }
        if i > 0 {
            out.push(' ');
        }
        // SAFETY: Kind names are static C strings.
        out.push_str(&unsafe { CStr::from_ptr(name) }.to_string_lossy());
        i += 1;
    }
    out.push(']');
    out
}

fn str_text(s: *const c_char) -> String {
    if s.is_null() {
        "NULL".to_owned()
    } else {
        // SAFETY: Non-null strings in the tables are static C strings.
        format!("{:?}", unsafe { CStr::from_ptr(s) })
    }
}

fn embedded_text(list: *const CbmEmbeddedLangSpec) -> String {
    if list.is_null() {
        return "NULL".to_owned();
    }
    let mut out = String::new();
    let mut i = 0;
    loop {
        // SAFETY: Every embedded list ends in an entry with a null script.
        let entry = unsafe { &*list.add(i) };
        if entry.script_node_type.is_null() {
            break;
        }
        if i > 0 {
            out.push(' ');
        }
        let _ = write!(
            out,
            "{} {} -> {};",
            str_text(entry.script_node_type),
            str_text(entry.content_node_type),
            entry.embedded_language
        );
        i += 1;
    }
    out
}

/// The dump of every accessor for one language value.
fn dump_lang(out: &mut String, lang: c_int) {
    let spec = cbm_lang_spec(lang);
    let grammar = cbm_ts_language(lang);
    let symbols = if grammar.is_null() {
        "none".to_owned()
    } else {
        // SAFETY: A non-null grammar is a static TSLanguage.
        unsafe { ts_language_symbol_count(grammar) }.to_string()
    };
    let _ = writeln!(
        out,
        "lang {lang}: grammar_symbols={symbols} conditional_block_types={}",
        kinds_text(cbm_conditional_block_types(lang))
    );
    if spec.is_null() {
        let _ = writeln!(out, "  spec NULL");
        return;
    }
    // SAFETY: A non-null spec points into the static table.
    let spec = unsafe { &*spec };
    let fields = [
        ("function_node_types", spec.function_node_types),
        ("class_node_types", spec.class_node_types),
        ("field_node_types", spec.field_node_types),
        ("module_node_types", spec.module_node_types),
        ("call_node_types", spec.call_node_types),
        ("import_node_types", spec.import_node_types),
        ("import_from_types", spec.import_from_types),
        ("branching_node_types", spec.branching_node_types),
        ("variable_node_types", spec.variable_node_types),
        ("assignment_node_types", spec.assignment_node_types),
        ("throw_node_types", spec.throw_node_types),
        ("decorator_node_types", spec.decorator_node_types),
        ("env_access_functions", spec.env_access_functions),
        (
            "env_access_member_patterns",
            spec.env_access_member_patterns,
        ),
    ];
    let _ = writeln!(out, "  language {}", spec.language);
    for (name, list) in fields {
        let _ = writeln!(out, "  {name} {}", kinds_text(list));
    }
    let _ = writeln!(
        out,
        "  throws_clause_field {}",
        str_text(spec.throws_clause_field)
    );
    let _ = writeln!(out, "  ts_factory {}", spec.ts_factory.is_some());
    let _ = writeln!(
        out,
        "  embedded_imports {}",
        embedded_text(spec.embedded_imports)
    );
}

fn dump() -> String {
    let mut out = String::new();
    for lang in [-1000, -2, -1].into_iter().chain(0..lang::COUNT).chain([
        lang::COUNT,
        lang::COUNT + 1,
        100_000,
    ]) {
        dump_lang(&mut out, lang);
    }
    out
}

#[test]
fn every_accessor_matches_the_golden_dump() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../internal/cbm/lang_specs.golden"
    );
    let actual = dump();
    if std::env::var_os("MORI_BLESS_LANG_SPECS").is_some() {
        std::fs::write(path, &actual).unwrap();
    }
    let expected = std::fs::read_to_string(path).unwrap();
    if actual != expected {
        let first = actual.lines().zip(expected.lines()).find(|(a, e)| a != e);
        panic!(
            "lang_specs tables differ from {path}; first differing line: \
             {first:?}. Rerun with MORI_BLESS_LANG_SPECS=1 for a deliberate \
             change and review the diff."
        );
    }
}

#[test]
fn rows_carry_their_own_language() {
    let mut rows = 0;
    for lang in 0..lang::COUNT {
        let spec = cbm_lang_spec(lang);
        if !spec.is_null() {
            // SAFETY: A non-null spec points into the static table.
            assert_eq!(unsafe { (*spec).language }, lang);
            rows += 1;
        }
    }
    assert!(rows > 150, "{rows} rows");
}
