//! Per-language Tree-sitter node-kind tables, served to C.
//!
//! This is the single source of truth for `CBMLangSpec` rows. C reads them
//! through `cbm_lang_spec`, `cbm_ts_language` and
//! `cbm_conditional_block_types`, declared in `lang_specs.h`; the layouts
//! below match that header.

use std::ffi::{CStr, c_char, c_int};
use std::ptr;

use crate::native_parser::RawLanguage;

type TsFactory = unsafe extern "C" fn() -> *const RawLanguage;

/// Matches `CBMEmbeddedLangSpec` in `lang_specs.h`.
#[repr(C)]
pub(crate) struct CbmEmbeddedLangSpec {
    script_node_type: *const c_char,
    content_node_type: *const c_char,
    embedded_language: c_int,
}

/// Matches `CBMLangSpec` in `lang_specs.h`. Every pointer names static,
/// immutable data, and each kind list ends in a null pointer.
#[repr(C)]
pub(crate) struct CbmLangSpec {
    language: c_int,
    function_node_types: *const *const c_char,
    class_node_types: *const *const c_char,
    field_node_types: *const *const c_char,
    module_node_types: *const *const c_char,
    call_node_types: *const *const c_char,
    import_node_types: *const *const c_char,
    import_from_types: *const *const c_char,
    branching_node_types: *const *const c_char,
    variable_node_types: *const *const c_char,
    assignment_node_types: *const *const c_char,
    throw_node_types: *const *const c_char,
    throws_clause_field: *const c_char,
    decorator_node_types: *const *const c_char,
    env_access_functions: *const *const c_char,
    env_access_member_patterns: *const *const c_char,
    ts_factory: Option<TsFactory>,
    embedded_imports: *const CbmEmbeddedLangSpec,
}

// SAFETY: the pointers name static, immutable C strings and arrays.
unsafe impl Sync for CbmEmbeddedLangSpec {}
// SAFETY: as above.
unsafe impl Sync for CbmLangSpec {}

/// A null-terminated array of node-kind names, laid out as C's
/// `const char *[]`.
#[repr(C)]
struct Kinds<const N: usize> {
    names: [*const c_char; N],
    end: *const c_char,
}

// SAFETY: the pointers name static, immutable C strings.
unsafe impl<const N: usize> Sync for Kinds<N> {}

impl<const N: usize> Kinds<N> {
    const fn as_ptr(&'static self) -> *const *const c_char {
        (self as *const Self).cast()
    }
}

const fn kinds<const N: usize>(names: [&'static CStr; N]) -> Kinds<N> {
    let mut ptrs = [ptr::null(); N];
    let mut i = 0;
    while i < N {
        ptrs[i] = names[i].as_ptr();
        i += 1;
    }
    Kinds {
        names: ptrs,
        end: ptr::null(),
    }
}

const fn embedded(
    script: &'static CStr,
    content: &'static CStr,
    language: c_int,
) -> CbmEmbeddedLangSpec {
    CbmEmbeddedLangSpec {
        script_node_type: script.as_ptr(),
        content_node_type: content.as_ptr(),
        embedded_language: language,
    }
}

/// Ends an embedded-language list, as `{NULL, NULL, 0}` does in C.
const EMBEDDED_END: CbmEmbeddedLangSpec = CbmEmbeddedLangSpec {
    script_node_type: ptr::null(),
    content_node_type: ptr::null(),
    embedded_language: 0,
};

/// `CBMLanguage` values from `cbm.h`, in enum order.
#[allow(dead_code)]
pub(crate) mod lang {
    use std::ffi::c_int;

    pub(crate) const GO: c_int = 0;
    pub(crate) const PYTHON: c_int = 1;
    pub(crate) const JAVASCRIPT: c_int = 2;
    pub(crate) const TYPESCRIPT: c_int = 3;
    pub(crate) const TSX: c_int = 4;
    pub(crate) const RUST: c_int = 5;
    pub(crate) const JAVA: c_int = 6;
    pub(crate) const CPP: c_int = 7;
    pub(crate) const CSHARP: c_int = 8;
    pub(crate) const PHP: c_int = 9;
    pub(crate) const LUA: c_int = 10;
    pub(crate) const SCALA: c_int = 11;
    pub(crate) const KOTLIN: c_int = 12;
    pub(crate) const RUBY: c_int = 13;
    pub(crate) const C: c_int = 14;
    pub(crate) const BASH: c_int = 15;
    pub(crate) const ZIG: c_int = 16;
    pub(crate) const ELIXIR: c_int = 17;
    pub(crate) const HASKELL: c_int = 18;
    pub(crate) const OCAML: c_int = 19;
    pub(crate) const OBJC: c_int = 20;
    pub(crate) const SWIFT: c_int = 21;
    pub(crate) const DART: c_int = 22;
    pub(crate) const PERL: c_int = 23;
    pub(crate) const GROOVY: c_int = 24;
    pub(crate) const ERLANG: c_int = 25;
    pub(crate) const R: c_int = 26;
    pub(crate) const HTML: c_int = 27;
    pub(crate) const CSS: c_int = 28;
    pub(crate) const SCSS: c_int = 29;
    pub(crate) const YAML: c_int = 30;
    pub(crate) const TOML: c_int = 31;
    pub(crate) const HCL: c_int = 32;
    pub(crate) const SQL: c_int = 33;
    pub(crate) const DOCKERFILE: c_int = 34;
    pub(crate) const CLOJURE: c_int = 35;
    pub(crate) const FSHARP: c_int = 36;
    pub(crate) const JULIA: c_int = 37;
    pub(crate) const VIMSCRIPT: c_int = 38;
    pub(crate) const NIX: c_int = 39;
    pub(crate) const COMMONLISP: c_int = 40;
    pub(crate) const ELM: c_int = 41;
    pub(crate) const FORTRAN: c_int = 42;
    pub(crate) const CUDA: c_int = 43;
    pub(crate) const COBOL: c_int = 44;
    pub(crate) const VERILOG: c_int = 45;
    pub(crate) const EMACSLISP: c_int = 46;
    pub(crate) const JSON: c_int = 47;
    pub(crate) const XML: c_int = 48;
    pub(crate) const MARKDOWN: c_int = 49;
    pub(crate) const MAKEFILE: c_int = 50;
    pub(crate) const CMAKE: c_int = 51;
    pub(crate) const PROTOBUF: c_int = 52;
    pub(crate) const GRAPHQL: c_int = 53;
    pub(crate) const VUE: c_int = 54;
    pub(crate) const SVELTE: c_int = 55;
    pub(crate) const MESON: c_int = 56;
    pub(crate) const GLSL: c_int = 57;
    pub(crate) const INI: c_int = 58;
    pub(crate) const MATLAB: c_int = 59;
    pub(crate) const LEAN: c_int = 60;
    pub(crate) const FORM: c_int = 61;
    pub(crate) const MAGMA: c_int = 62;
    pub(crate) const WOLFRAM: c_int = 63;
    pub(crate) const SOLIDITY: c_int = 64;
    pub(crate) const TYPST: c_int = 65;
    pub(crate) const GDSCRIPT: c_int = 66;
    pub(crate) const GLEAM: c_int = 67;
    pub(crate) const POWERSHELL: c_int = 68;
    pub(crate) const PASCAL: c_int = 69;
    pub(crate) const DLANG: c_int = 70;
    pub(crate) const NIM: c_int = 71;
    pub(crate) const SCHEME: c_int = 72;
    pub(crate) const FENNEL: c_int = 73;
    pub(crate) const FISH: c_int = 74;
    pub(crate) const AWK: c_int = 75;
    pub(crate) const ZSH: c_int = 76;
    pub(crate) const TCL: c_int = 77;
    pub(crate) const ADA: c_int = 78;
    pub(crate) const AGDA: c_int = 79;
    pub(crate) const RACKET: c_int = 80;
    pub(crate) const ODIN: c_int = 81;
    pub(crate) const RESCRIPT: c_int = 82;
    pub(crate) const PURESCRIPT: c_int = 83;
    pub(crate) const NICKEL: c_int = 84;
    pub(crate) const CRYSTAL: c_int = 85;
    pub(crate) const TEAL: c_int = 86;
    pub(crate) const HARE: c_int = 87;
    pub(crate) const PONY: c_int = 88;
    pub(crate) const LUAU: c_int = 89;
    pub(crate) const JANET: c_int = 90;
    pub(crate) const SWAY: c_int = 91;
    pub(crate) const NASM: c_int = 92;
    pub(crate) const ASSEMBLY: c_int = 93;
    pub(crate) const ASTRO: c_int = 94;
    pub(crate) const BLADE: c_int = 95;
    pub(crate) const JUST: c_int = 96;
    pub(crate) const GOTEMPLATE: c_int = 97;
    pub(crate) const TEMPL: c_int = 98;
    pub(crate) const LIQUID: c_int = 99;
    pub(crate) const JINJA2: c_int = 100;
    pub(crate) const PRISMA: c_int = 101;
    pub(crate) const HYPRLANG: c_int = 102;
    pub(crate) const DOTENV: c_int = 103;
    pub(crate) const DIFF: c_int = 104;
    pub(crate) const WGSL: c_int = 105;
    pub(crate) const KDL: c_int = 106;
    pub(crate) const JSON5: c_int = 107;
    pub(crate) const JSONNET: c_int = 108;
    pub(crate) const RON: c_int = 109;
    pub(crate) const THRIFT: c_int = 110;
    pub(crate) const CAPNP: c_int = 111;
    pub(crate) const PROPERTIES: c_int = 112;
    pub(crate) const SSHCONFIG: c_int = 113;
    pub(crate) const BIBTEX: c_int = 114;
    pub(crate) const STARLARK: c_int = 115;
    pub(crate) const BICEP: c_int = 116;
    pub(crate) const CSV: c_int = 117;
    pub(crate) const REQUIREMENTS: c_int = 118;
    pub(crate) const HLSL: c_int = 119;
    pub(crate) const VHDL: c_int = 120;
    pub(crate) const SYSTEMVERILOG: c_int = 121;
    pub(crate) const DEVICETREE: c_int = 122;
    pub(crate) const LINKERSCRIPT: c_int = 123;
    pub(crate) const GN: c_int = 124;
    pub(crate) const KCONFIG: c_int = 125;
    pub(crate) const BITBAKE: c_int = 126;
    pub(crate) const SMALI: c_int = 127;
    pub(crate) const TABLEGEN: c_int = 128;
    pub(crate) const ISPC: c_int = 129;
    pub(crate) const CAIRO: c_int = 130;
    pub(crate) const MOVE: c_int = 131;
    pub(crate) const SQUIRREL: c_int = 132;
    pub(crate) const FUNC: c_int = 133;
    pub(crate) const REGEX: c_int = 134;
    pub(crate) const JSDOC: c_int = 135;
    pub(crate) const RST: c_int = 136;
    pub(crate) const BEANCOUNT: c_int = 137;
    pub(crate) const MERMAID: c_int = 138;
    pub(crate) const PUPPET: c_int = 139;
    pub(crate) const PO: c_int = 140;
    pub(crate) const GITATTRIBUTES: c_int = 141;
    pub(crate) const GITIGNORE: c_int = 142;
    pub(crate) const SLANG: c_int = 143;
    pub(crate) const LLVM_IR: c_int = 144;
    pub(crate) const SMITHY: c_int = 145;
    pub(crate) const WIT: c_int = 146;
    pub(crate) const TLAPLUS: c_int = 147;
    pub(crate) const PKL: c_int = 148;
    pub(crate) const GOMOD: c_int = 149;
    pub(crate) const APEX: c_int = 150;
    pub(crate) const SOQL: c_int = 151;
    pub(crate) const SOSL: c_int = 152;
    pub(crate) const KUSTOMIZE: c_int = 153;
    pub(crate) const K8S: c_int = 154;
    pub(crate) const PINE: c_int = 155;
    pub(crate) const QML: c_int = 156;
    pub(crate) const CFSCRIPT: c_int = 157;
    pub(crate) const CFML: c_int = 158;
    pub(crate) const MOJO: c_int = 159;
    pub(crate) const OBJECTSCRIPT_UDL: c_int = 160;
    pub(crate) const OBJECTSCRIPT_ROUTINE: c_int = 161;
    pub(crate) const OBJECTSCRIPT_EXPORT: c_int = 162;
    pub(crate) const ARKTS: c_int = 163;
    pub(crate) const PLSQL: c_int = 164;
    pub(crate) const CHIALISP: c_int = 165;
    pub(crate) const PDF: c_int = 166;
    pub(crate) const ASCIIDOC: c_int = 167;
    pub(crate) const OPENAPI: c_int = 168;
    pub(crate) const DIAGRAM: c_int = 169;
    pub(crate) const COUNT: c_int = 170;
}

const LANG_COUNT: usize = lang::COUNT as usize;

// Grammar factories, defined by the vendored grammar objects.
unsafe extern "C" {
    fn tree_sitter_go() -> *const RawLanguage;
    fn tree_sitter_python() -> *const RawLanguage;
    fn tree_sitter_javascript() -> *const RawLanguage;
    fn tree_sitter_typescript() -> *const RawLanguage;
    fn tree_sitter_tsx() -> *const RawLanguage;
    fn tree_sitter_rust() -> *const RawLanguage;
    fn tree_sitter_java() -> *const RawLanguage;
    fn tree_sitter_cpp() -> *const RawLanguage;
    fn tree_sitter_c_sharp() -> *const RawLanguage;
    fn tree_sitter_php_only() -> *const RawLanguage;
    fn tree_sitter_lua() -> *const RawLanguage;
    fn tree_sitter_scala() -> *const RawLanguage;
    fn tree_sitter_kotlin() -> *const RawLanguage;
    fn tree_sitter_ruby() -> *const RawLanguage;
    fn tree_sitter_c() -> *const RawLanguage;
    fn tree_sitter_bash() -> *const RawLanguage;
    fn tree_sitter_zig() -> *const RawLanguage;
    fn tree_sitter_elixir() -> *const RawLanguage;
    fn tree_sitter_haskell() -> *const RawLanguage;
    fn tree_sitter_ocaml() -> *const RawLanguage;
    fn tree_sitter_objc() -> *const RawLanguage;
    fn tree_sitter_swift() -> *const RawLanguage;
    fn tree_sitter_dart() -> *const RawLanguage;
    fn tree_sitter_perl() -> *const RawLanguage;
    fn tree_sitter_groovy() -> *const RawLanguage;
    fn tree_sitter_erlang() -> *const RawLanguage;
    fn tree_sitter_r() -> *const RawLanguage;
    fn tree_sitter_html() -> *const RawLanguage;
    fn tree_sitter_css() -> *const RawLanguage;
    fn tree_sitter_scss() -> *const RawLanguage;
    fn tree_sitter_yaml() -> *const RawLanguage;
    fn tree_sitter_toml() -> *const RawLanguage;
    fn tree_sitter_hcl() -> *const RawLanguage;
    fn tree_sitter_sql() -> *const RawLanguage;
    fn tree_sitter_dockerfile() -> *const RawLanguage;
    fn tree_sitter_clojure() -> *const RawLanguage;
    fn tree_sitter_fsharp() -> *const RawLanguage;
    fn tree_sitter_julia() -> *const RawLanguage;
    fn tree_sitter_vim() -> *const RawLanguage;
    fn tree_sitter_nix() -> *const RawLanguage;
    fn tree_sitter_commonlisp() -> *const RawLanguage;
    fn tree_sitter_elm() -> *const RawLanguage;
    fn tree_sitter_fortran() -> *const RawLanguage;
    fn tree_sitter_cuda() -> *const RawLanguage;
    fn tree_sitter_COBOL() -> *const RawLanguage;
    fn tree_sitter_verilog() -> *const RawLanguage;
    fn tree_sitter_elisp() -> *const RawLanguage;
    fn tree_sitter_json() -> *const RawLanguage;
    fn tree_sitter_xml() -> *const RawLanguage;
    fn tree_sitter_markdown() -> *const RawLanguage;
    fn tree_sitter_make() -> *const RawLanguage;
    fn tree_sitter_cmake() -> *const RawLanguage;
    fn tree_sitter_proto() -> *const RawLanguage;
    fn tree_sitter_graphql() -> *const RawLanguage;
    fn tree_sitter_vue() -> *const RawLanguage;
    fn tree_sitter_svelte() -> *const RawLanguage;
    fn tree_sitter_meson() -> *const RawLanguage;
    fn tree_sitter_glsl() -> *const RawLanguage;
    fn tree_sitter_ini() -> *const RawLanguage;
    fn tree_sitter_matlab() -> *const RawLanguage;
    fn tree_sitter_lean() -> *const RawLanguage;
    fn tree_sitter_form() -> *const RawLanguage;
    fn tree_sitter_magma() -> *const RawLanguage;
    fn tree_sitter_wolfram() -> *const RawLanguage;
    fn tree_sitter_solidity() -> *const RawLanguage;
    fn tree_sitter_typst() -> *const RawLanguage;
    fn tree_sitter_gdscript() -> *const RawLanguage;
    fn tree_sitter_gleam() -> *const RawLanguage;
    fn tree_sitter_powershell() -> *const RawLanguage;
    fn tree_sitter_pascal() -> *const RawLanguage;
    fn tree_sitter_d() -> *const RawLanguage;
    fn tree_sitter_scheme() -> *const RawLanguage;
    fn tree_sitter_chialisp() -> *const RawLanguage;
    fn tree_sitter_fennel() -> *const RawLanguage;
    fn tree_sitter_fish() -> *const RawLanguage;
    fn tree_sitter_awk() -> *const RawLanguage;
    fn tree_sitter_zsh() -> *const RawLanguage;
    fn tree_sitter_tcl() -> *const RawLanguage;
    fn tree_sitter_ada() -> *const RawLanguage;
    fn tree_sitter_agda() -> *const RawLanguage;
    fn tree_sitter_racket() -> *const RawLanguage;
    fn tree_sitter_odin() -> *const RawLanguage;
    fn tree_sitter_rescript() -> *const RawLanguage;
    fn tree_sitter_purescript() -> *const RawLanguage;
    fn tree_sitter_nickel() -> *const RawLanguage;
    fn tree_sitter_crystal() -> *const RawLanguage;
    fn tree_sitter_teal() -> *const RawLanguage;
    fn tree_sitter_hare() -> *const RawLanguage;
    fn tree_sitter_pony() -> *const RawLanguage;
    fn tree_sitter_luau() -> *const RawLanguage;
    fn tree_sitter_qmljs() -> *const RawLanguage;
    fn tree_sitter_cfscript() -> *const RawLanguage;
    fn tree_sitter_cfml() -> *const RawLanguage;
    fn tree_sitter_janet_simple() -> *const RawLanguage;
    fn tree_sitter_sway() -> *const RawLanguage;
    fn tree_sitter_nasm() -> *const RawLanguage;
    fn tree_sitter_asm() -> *const RawLanguage;
    fn tree_sitter_astro() -> *const RawLanguage;
    fn tree_sitter_blade() -> *const RawLanguage;
    fn tree_sitter_just() -> *const RawLanguage;
    fn tree_sitter_gotmpl() -> *const RawLanguage;
    fn tree_sitter_templ() -> *const RawLanguage;
    fn tree_sitter_liquid() -> *const RawLanguage;
    fn tree_sitter_jinja2() -> *const RawLanguage;
    fn tree_sitter_prisma() -> *const RawLanguage;
    fn tree_sitter_hyprlang() -> *const RawLanguage;
    fn tree_sitter_dotenv() -> *const RawLanguage;
    fn tree_sitter_diff() -> *const RawLanguage;
    fn tree_sitter_wgsl() -> *const RawLanguage;
    fn tree_sitter_kdl() -> *const RawLanguage;
    fn tree_sitter_json5() -> *const RawLanguage;
    fn tree_sitter_jsonnet() -> *const RawLanguage;
    fn tree_sitter_ron() -> *const RawLanguage;
    fn tree_sitter_thrift() -> *const RawLanguage;
    fn tree_sitter_capnp() -> *const RawLanguage;
    fn tree_sitter_properties() -> *const RawLanguage;
    fn tree_sitter_ssh_config() -> *const RawLanguage;
    fn tree_sitter_bibtex() -> *const RawLanguage;
    fn tree_sitter_starlark() -> *const RawLanguage;
    fn tree_sitter_bicep() -> *const RawLanguage;
    fn tree_sitter_csv() -> *const RawLanguage;
    fn tree_sitter_requirements() -> *const RawLanguage;
    fn tree_sitter_hlsl() -> *const RawLanguage;
    fn tree_sitter_vhdl() -> *const RawLanguage;
    fn tree_sitter_systemverilog() -> *const RawLanguage;
    fn tree_sitter_devicetree() -> *const RawLanguage;
    fn tree_sitter_linkerscript() -> *const RawLanguage;
    fn tree_sitter_gn() -> *const RawLanguage;
    fn tree_sitter_kconfig() -> *const RawLanguage;
    fn tree_sitter_bitbake() -> *const RawLanguage;
    fn tree_sitter_smali() -> *const RawLanguage;
    fn tree_sitter_tablegen() -> *const RawLanguage;
    fn tree_sitter_ispc() -> *const RawLanguage;
    fn tree_sitter_cairo() -> *const RawLanguage;
    fn tree_sitter_move() -> *const RawLanguage;
    fn tree_sitter_squirrel() -> *const RawLanguage;
    fn tree_sitter_func() -> *const RawLanguage;
    fn tree_sitter_regex() -> *const RawLanguage;
    fn tree_sitter_jsdoc() -> *const RawLanguage;
    fn tree_sitter_rst() -> *const RawLanguage;
    fn tree_sitter_beancount() -> *const RawLanguage;
    fn tree_sitter_mermaid() -> *const RawLanguage;
    fn tree_sitter_puppet() -> *const RawLanguage;
    fn tree_sitter_po() -> *const RawLanguage;
    fn tree_sitter_gitattributes() -> *const RawLanguage;
    fn tree_sitter_gitignore() -> *const RawLanguage;
    fn tree_sitter_slang() -> *const RawLanguage;
    fn tree_sitter_llvm() -> *const RawLanguage;
    fn tree_sitter_smithy() -> *const RawLanguage;
    fn tree_sitter_wit() -> *const RawLanguage;
    fn tree_sitter_tlaplus() -> *const RawLanguage;
    fn tree_sitter_pkl() -> *const RawLanguage;
    fn tree_sitter_gomod() -> *const RawLanguage;
    fn tree_sitter_apex() -> *const RawLanguage;
    fn tree_sitter_soql() -> *const RawLanguage;
    fn tree_sitter_sosl() -> *const RawLanguage;
    fn tree_sitter_pine() -> *const RawLanguage;
    fn tree_sitter_mojo() -> *const RawLanguage;
    fn tree_sitter_objectscript_udl() -> *const RawLanguage;
    fn tree_sitter_objectscript_routine() -> *const RawLanguage;
    fn tree_sitter_arkts() -> *const RawLanguage;
    fn tree_sitter_plsql() -> *const RawLanguage;
}

// The empty set: a language without nodes of this kind.
static EMPTY_TYPES: Kinds<0> = kinds([]);

// ==================== GO ====================
static GO_FUNC_TYPES: Kinds<4> = kinds([
    c"function_declaration",
    c"method_declaration",
    c"method_elem",
    c"func_literal",
]);
static GO_CLASS_TYPES: Kinds<3> =
    kinds([c"type_spec", c"type_alias", c"type_declaration"]);
static GO_FIELD_TYPES: Kinds<1> = kinds([c"field_declaration"]);
static GO_MODULE_TYPES: Kinds<1> = kinds([c"source_file"]);
static GO_CALL_TYPES: Kinds<1> = kinds([c"call_expression"]);
static GO_IMPORT_TYPES: Kinds<2> = kinds([c"import_declaration", c"import"]);
static GO_BRANCH_TYPES: Kinds<11> = kinds([
    c"if_statement",
    c"for_statement",
    c"expression_switch_statement",
    c"type_switch_statement",
    c"select_statement",
    c"expression_case",
    c"type_case",
    c"communication_case",
    c"default_case",
    c"defer_statement",
    c"go_statement",
]);
static GO_VAR_TYPES: Kinds<2> =
    kinds([c"var_declaration", c"const_declaration"]);
static GO_ASSIGN_TYPES: Kinds<2> =
    kinds([c"assignment_statement", c"short_var_declaration"]);

// ==================== PYTHON ====================
static PY_FUNC_TYPES: Kinds<1> = kinds([c"function_definition"]);
static PY_CLASS_TYPES: Kinds<1> = kinds([c"class_definition"]);
static PY_MODULE_TYPES: Kinds<1> = kinds([c"module"]);
static PY_CALL_TYPES: Kinds<1> = kinds([c"call"]);
static PY_IMPORT_TYPES: Kinds<2> = kinds([c"import_statement", c"import"]);
static PY_IMPORT_FROM_TYPES: Kinds<2> =
    kinds([c"import_from_statement", c"future_import_statement"]);
static PY_BRANCH_TYPES: Kinds<7> = kinds([
    c"if_statement",
    c"for_statement",
    c"while_statement",
    c"try_statement",
    c"except_clause",
    c"with_statement",
    c"elif_clause",
]);
static PY_VAR_TYPES: Kinds<2> = kinds([c"assignment", c"augmented_assignment"]);
static PY_THROW_TYPES: Kinds<1> = kinds([c"raise_statement"]);
static PY_DECORATOR_TYPES: Kinds<1> = kinds([c"decorator"]);

// ==================== JAVASCRIPT ====================
static JS_FUNC_TYPES: Kinds<5> = kinds([
    c"function_declaration",
    c"generator_function_declaration",
    c"function_expression",
    c"arrow_function",
    c"method_definition",
]);
static JS_CLASS_TYPES: Kinds<2> = kinds([c"class_declaration", c"class"]);
static JS_MODULE_TYPES: Kinds<1> = kinds([c"program"]);
static JS_CALL_TYPES: Kinds<2> = kinds([c"call_expression", c"new_expression"]);
static JS_IMPORT_TYPES: Kinds<5> = kinds([
    c"import_statement",
    c"export_statement",
    c"import",
    c"extends",
    c"require",
]);
static JS_BRANCH_TYPES: Kinds<10> = kinds([
    c"if_statement",
    c"for_statement",
    c"for_in_statement",
    c"while_statement",
    c"switch_statement",
    c"switch_case",
    c"switch_default",
    c"try_statement",
    c"catch_clause",
    c"do_statement",
]);
static JS_VAR_TYPES: Kinds<2> =
    kinds([c"lexical_declaration", c"variable_declaration"]);
static JS_THROW_TYPES: Kinds<1> = kinds([c"throw_statement"]);

// ==================== TYPESCRIPT ====================
static TS_FUNC_TYPES: Kinds<6> = kinds([
    c"function_declaration",
    c"generator_function_declaration",
    c"function_expression",
    c"arrow_function",
    c"method_definition",
    c"function_signature",
]);
static TS_CLASS_TYPES: Kinds<7> = kinds([
    c"class_declaration",
    c"class",
    c"abstract_class_declaration",
    c"enum_declaration",
    c"interface_declaration",
    c"type_alias_declaration",
    c"internal_module",
]);
static TS_DECORATOR_TYPES: Kinds<1> = kinds([c"decorator"]);

// ==================== ARKTS (HarmonyOS .ets) ====================
// TypeScript-superset grammar (first-party tree-sitter-typescript fork).
// Reuses the JS/TS arrays and adds the ArkUI constructs: `@Component struct`
// components are class-like containers (struct_declaration), and their
// decorated state members (public_field_definition) are extracted as fields
// so @State/@Prop/@Link/... properties are findable.
static ARKTS_CLASS_TYPES: Kinds<8> = kinds([
    c"class_declaration",
    c"class",
    c"abstract_class_declaration",
    c"struct_declaration",
    c"enum_declaration",
    c"interface_declaration",
    c"type_alias_declaration",
    c"internal_module",
]);
static ARKTS_FIELD_TYPES: Kinds<1> = kinds([c"public_field_definition"]);

// ==================== QML (Qt) ====================
// QMLJS grammar is a TypeScript superset plus declarative ui_* nodes, so the
// JS/TS function, call, branch, variable and module arrays are reused as-is.
static QML_CLASS_TYPES: Kinds<6> = kinds([
    c"class_declaration",
    c"class",
    c"abstract_class_declaration",
    c"enum_declaration",
    c"interface_declaration",
    c"ui_inline_component",
]);
static QML_FIELD_TYPES: Kinds<3> =
    kinds([c"ui_property", c"ui_signal", c"public_field_definition"]);
static QML_IMPORT_TYPES: Kinds<3> =
    kinds([c"import_statement", c"import", c"ui_import"]);

// ==================== CFScript (CFML .cfc script dialect) ====================
// JS-like grammar: components contain function/method declarations. Reuses the
// JS call/branch/var/module arrays.
static CFSCRIPT_FUNC_TYPES: Kinds<4> = kinds([
    c"function_declaration",
    c"function_expression",
    c"arrow_function",
    c"method_definition",
]);
static CFSCRIPT_FIELD_TYPES: Kinds<1> = kinds([c"property_declaration"]);
static CFSCRIPT_IMPORT_TYPES: Kinds<2> = kinds([c"import_statement", c"import"]);

// ==================== CFML (tag dialect — .cfm templates) ====================
// Tag-based grammar (HTML-derived). Embedded <cfscript> functions appear as
// function_declaration/function_expression. Tag <cffunction> nodes
// (cf_function_tag) carry their name in a cf_attribute rather than a `name`
// field, so the definition walker mints them via extract_cfml_function_tag and
// compute_func_qn names them via compute_cfml_func_qn — but cf_function_tag is
// listed here too so push_boundary_scopes pushes a SCOPE_FUNC and in-body calls
// source to the enclosing cffunction rather than the Module.
static CFML_FUNC_TYPES: Kinds<3> = kinds([
    c"cf_function_tag",
    c"function_declaration",
    c"function_expression",
]);
static CFML_CALL_TYPES: Kinds<1> = kinds([c"call_expression"]);
static CFML_BRANCH_TYPES: Kinds<7> = kinds([
    c"cf_if_tag",
    c"cf_elseif_tag",
    c"cf_else_tag",
    c"if_statement",
    c"for_statement",
    c"while_statement",
    c"switch_statement",
]);
static CFML_MODULE_TYPES: Kinds<2> = kinds([c"program", c"component_file"]);

// ==================== RUST ====================
static RUST_FUNC_TYPES: Kinds<3> = kinds([
    c"function_item",
    c"function_signature_item",
    c"closure_expression",
]);
static RUST_CLASS_TYPES: Kinds<6> = kinds([
    c"struct_item",
    c"enum_item",
    c"union_item",
    c"trait_item",
    c"type_item",
    c"impl_item",
]);
static RUST_FIELD_TYPES: Kinds<1> = kinds([c"field_declaration"]);
static RUST_MODULE_TYPES: Kinds<2> = kinds([c"source_file", c"mod_item"]);
static RUST_CALL_TYPES: Kinds<2> =
    kinds([c"call_expression", c"macro_invocation"]);
static RUST_IMPORT_TYPES: Kinds<2> =
    kinds([c"use_declaration", c"extern_crate_declaration"]);
static RUST_IMPORT_FROM_TYPES: Kinds<1> = kinds([c"use_declaration"]);
static RUST_BRANCH_TYPES: Kinds<6> = kinds([
    c"if_expression",
    c"for_expression",
    c"while_expression",
    c"loop_expression",
    c"match_expression",
    c"match_arm",
]);
static RUST_VAR_TYPES: Kinds<2> = kinds([c"static_item", c"const_item"]);
static RUST_ASSIGN_TYPES: Kinds<2> =
    kinds([c"assignment_expression", c"compound_assignment_expr"]);
static RUST_DECORATOR_TYPES: Kinds<1> = kinds([c"attribute_item"]);

// ==================== JAVA ====================
static JAVA_FUNC_TYPES: Kinds<3> = kinds([
    c"method_declaration",
    c"constructor_declaration",
    c"lambda_expression",
]);
static JAVA_CLASS_TYPES: Kinds<7> = kinds([
    c"class_declaration",
    c"interface_declaration",
    c"enum_declaration",
    c"annotation_type_declaration",
    c"record_declaration",
    c"module_declaration",
    c"package_declaration",
]);
static JAVA_FIELD_TYPES: Kinds<1> = kinds([c"field_declaration"]);
static JAVA_MODULE_TYPES: Kinds<1> = kinds([c"program"]);
static JAVA_CALL_TYPES: Kinds<2> =
    kinds([c"method_invocation", c"object_creation_expression"]);
static JAVA_IMPORT_TYPES: Kinds<3> =
    kinds([c"import_declaration", c"extends", c"import"]);
static JAVA_BRANCH_TYPES: Kinds<8> = kinds([
    c"if_statement",
    c"for_statement",
    c"enhanced_for_statement",
    c"while_statement",
    c"switch_expression",
    c"switch_block_statement_group",
    c"try_statement",
    c"catch_clause",
]);
static JAVA_VAR_TYPES: Kinds<2> =
    kinds([c"field_declaration", c"local_variable_declaration"]);
static JAVA_ASSIGN_TYPES: Kinds<1> = kinds([c"assignment_expression"]);
static JAVA_THROW_TYPES: Kinds<1> = kinds([c"throw_statement"]);
static JAVA_DECORATOR_TYPES: Kinds<2> =
    kinds([c"marker_annotation", c"annotation"]);

// ==================== C++ ====================
static CPP_FUNC_TYPES: Kinds<5> = kinds([
    c"function_definition",
    c"declaration",
    c"field_declaration",
    c"template_declaration",
    c"lambda_expression",
]);
static CPP_CLASS_TYPES: Kinds<5> = kinds([
    c"class_specifier",
    c"struct_specifier",
    c"union_specifier",
    c"enum_specifier",
    c"type_definition",
]);
static CPP_FIELD_TYPES: Kinds<1> = kinds([c"field_declaration"]);
static CPP_MODULE_TYPES: Kinds<4> = kinds([
    c"translation_unit",
    c"namespace_definition",
    c"linkage_specification",
    c"declaration",
]);
static CPP_CALL_TYPES: Kinds<2> = kinds([c"call_expression", c"new_expression"]);
static CPP_IMPORT_TYPES: Kinds<1> = kinds([c"preproc_include"]);
static CPP_BRANCH_TYPES: Kinds<8> = kinds([
    c"if_statement",
    c"for_statement",
    c"for_range_loop",
    c"while_statement",
    c"switch_statement",
    c"case_statement",
    c"try_statement",
    c"catch_clause",
]);
static CPP_VAR_TYPES: Kinds<1> = kinds([c"declaration"]);

// No update_expression here, unlike C: a bare C++ name can be an implicit-this
// member, so a local counter's `i++` would bind by name to an unrelated member.
static CPP_ASSIGN_TYPES: Kinds<1> = kinds([c"assignment_expression"]);
static CPP_THROW_TYPES: Kinds<1> = kinds([c"throw_statement"]);

// ==================== C# ====================
static CS_FUNC_TYPES: Kinds<8> = kinds([
    c"destructor_declaration",
    c"local_function_statement",
    c"function_pointer_type",
    c"constructor_declaration",
    c"anonymous_method_expression",
    c"lambda_expression",
    c"method_declaration",
    c"operator_declaration",
]);
static CS_CLASS_TYPES: Kinds<6> = kinds([
    c"class_declaration",
    c"struct_declaration",
    c"enum_declaration",
    c"interface_declaration",
    c"record_declaration",
    c"type_declaration",
]);
static CS_MODULE_TYPES: Kinds<1> = kinds([c"compilation_unit"]);
static CS_CALL_TYPES: Kinds<2> =
    kinds([c"invocation_expression", c"object_creation_expression"]);
static CS_IMPORT_TYPES: Kinds<4> = kinds([
    c"using_directive",
    c"namespace_use_declaration",
    c"using_statement",
    c"namespace_declaration",
]);
static CS_BRANCH_TYPES: Kinds<8> = kinds([
    c"if_statement",
    c"for_statement",
    c"foreach_statement",
    c"while_statement",
    c"switch_statement",
    c"case_switch_label",
    c"try_statement",
    c"catch_clause",
]);
static CS_VAR_TYPES: Kinds<2> =
    kinds([c"field_declaration", c"local_declaration_statement"]);
static CS_FIELD_TYPES: Kinds<2> =
    kinds([c"field_declaration", c"property_declaration"]);

// tree-sitter-c-sharp models `x++`/`++x` as postfix_/prefix_unary_expression
// (there is no `update_expression` node — that is a JS/TS kind), so a static
// field bump like `_count++` must list those node kinds to emit a WRITES.
static CS_ASSIGN_TYPES: Kinds<3> = kinds([
    c"assignment_expression",
    c"postfix_unary_expression",
    c"prefix_unary_expression",
]);
static CS_THROW_TYPES: Kinds<2> =
    kinds([c"throw_statement", c"throw_expression"]);
static CS_DECORATOR_TYPES: Kinds<1> = kinds([c"attribute"]);

// ==================== PHP ====================
static PHP_FUNC_TYPES: Kinds<5> = kinds([
    c"function_static_declaration",
    c"anonymous_function",
    c"function_definition",
    c"arrow_function",
    c"method_declaration",
]);
static PHP_CLASS_TYPES: Kinds<4> = kinds([
    c"trait_declaration",
    c"enum_declaration",
    c"interface_declaration",
    c"class_declaration",
]);
static PHP_IMPORT_TYPES: Kinds<5> = kinds([
    c"extends",
    c"include",
    c"namespace_use_declaration",
    c"require",
    c"use_declaration",
]);
static PHP_MODULE_TYPES: Kinds<1> = kinds([c"program"]);
static PHP_CALL_TYPES: Kinds<5> = kinds([
    c"member_call_expression",
    c"scoped_call_expression",
    c"function_call_expression",
    c"object_creation_expression",
    c"nullsafe_member_call_expression",
]);
static PHP_BRANCH_TYPES: Kinds<8> = kinds([
    c"if_statement",
    c"for_statement",
    c"foreach_statement",
    c"while_statement",
    c"switch_statement",
    c"case_statement",
    c"try_statement",
    c"catch_clause",
]);
static PHP_VAR_TYPES: Kinds<1> = kinds([c"expression_statement"]);
static PHP_ASSIGN_TYPES: Kinds<1> = kinds([c"assignment_expression"]);
static PHP_THROW_TYPES: Kinds<1> = kinds([c"throw_expression"]);
static PHP_DECORATOR_TYPES: Kinds<1> = kinds([c"attribute_group"]);

// ==================== LUA ====================
static LUA_FUNC_TYPES: Kinds<2> =
    kinds([c"function_declaration", c"function_definition"]);
static LUA_MODULE_TYPES: Kinds<1> = kinds([c"chunk"]);
static LUA_CALL_TYPES: Kinds<1> = kinds([c"function_call"]);
static LUA_IMPORT_TYPES: Kinds<1> = kinds([c"function_call"]);
static LUA_BRANCH_TYPES: Kinds<5> = kinds([
    c"if_statement",
    c"for_statement",
    c"for_in_statement",
    c"while_statement",
    c"repeat_statement",
]);
static LUA_VAR_TYPES: Kinds<1> = kinds([c"variable_declaration"]);
static LUA_ASSIGN_TYPES: Kinds<1> = kinds([c"assignment_statement"]);

// ==================== SCALA ====================
static SCALA_FUNC_TYPES: Kinds<3> = kinds([
    c"function_definition",
    c"function_declaration",
    c"lambda_expression",
]);
static SCALA_CLASS_TYPES: Kinds<5> = kinds([
    c"class_definition",
    c"object_definition",
    c"trait_definition",
    c"enum_definition",
    c"type_definition",
]);
static SCALA_MODULE_TYPES: Kinds<1> = kinds([c"compilation_unit"]);
static SCALA_CALL_TYPES: Kinds<3> = kinds([
    c"call_expression",
    c"infix_expression",
    c"instance_expression",
]);
static SCALA_IMPORT_TYPES: Kinds<4> = kinds([
    c"import_declaration",
    c"extends",
    c"import",
    c"using_directive",
]);
static SCALA_BRANCH_TYPES: Kinds<7> = kinds([
    c"if_expression",
    c"for_expression",
    c"while_expression",
    c"match_expression",
    c"case_clause",
    c"try_expression",
    c"catch_clause",
]);
static SCALA_VAR_TYPES: Kinds<4> = kinds([
    c"val_definition",
    c"var_definition",
    c"val_declaration",
    c"var_declaration",
]);
static SCALA_ASSIGN_TYPES: Kinds<1> = kinds([c"assignment_expression"]);
static SCALA_THROW_TYPES: Kinds<1> = kinds([c"throw_expression"]);
static SCALA_DECORATOR_TYPES: Kinds<1> = kinds([c"annotation"]);

// ==================== KOTLIN ====================
static KOTLIN_FUNC_TYPES: Kinds<3> = kinds([
    c"function_declaration",
    c"secondary_constructor",
    c"anonymous_function",
]);
static KOTLIN_CLASS_TYPES: Kinds<4> = kinds([
    c"class_declaration",
    c"object_declaration",
    c"companion_object",
    c"type_alias",
]);
static KOTLIN_MODULE_TYPES: Kinds<1> = kinds([c"source_file"]);
static KOTLIN_CALL_TYPES: Kinds<1> = kinds([c"call_expression"]);
static KOTLIN_IMPORT_TYPES: Kinds<1> = kinds([c"import"]);
static KOTLIN_BRANCH_TYPES: Kinds<7> = kinds([
    c"if_expression",
    c"for_statement",
    c"while_statement",
    c"when_expression",
    c"when_entry",
    c"try_expression",
    c"catch_block",
]);
static KOTLIN_VAR_TYPES: Kinds<1> = kinds([c"property_declaration"]);
static KOTLIN_ASSIGN_TYPES: Kinds<2> =
    kinds([c"assignment", c"directly_assignable_expression"]);
static KOTLIN_THROW_TYPES: Kinds<1> = kinds([c"throw_expression"]);
static KOTLIN_DECORATOR_TYPES: Kinds<1> = kinds([c"annotation"]);

// ==================== RUBY ====================
static RUBY_FUNC_TYPES: Kinds<2> = kinds([c"method", c"singleton_method"]);
static RUBY_CLASS_TYPES: Kinds<2> = kinds([c"class", c"module"]);
static RUBY_MODULE_TYPES: Kinds<1> = kinds([c"program"]);
static RUBY_CALL_TYPES: Kinds<1> = kinds([c"call"]);
static RUBY_IMPORT_TYPES: Kinds<1> = kinds([c"call"]);
static RUBY_BRANCH_TYPES: Kinds<9> = kinds([
    c"if", c"unless", c"while", c"until", c"for", c"case", c"when", c"rescue",
    c"elsif",
]);
static RUBY_VAR_TYPES: Kinds<1> = kinds([c"assignment"]);
static RUBY_ASSIGN_TYPES: Kinds<2> =
    kinds([c"assignment", c"operator_assignment"]);

// ==================== C ====================
static C_FUNC_TYPES: Kinds<1> = kinds([c"function_definition"]);
static C_CLASS_TYPES: Kinds<4> = kinds([
    c"struct_specifier",
    c"enum_specifier",
    c"union_specifier",
    c"type_definition",
]);
static C_FIELD_TYPES: Kinds<1> = kinds([c"field_declaration"]);
static C_MODULE_TYPES: Kinds<1> = kinds([c"translation_unit"]);
static C_CALL_TYPES: Kinds<1> = kinds([c"call_expression"]);
static C_IMPORT_TYPES: Kinds<1> = kinds([c"preproc_include"]);
static C_BRANCH_TYPES: Kinds<6> = kinds([
    c"if_statement",
    c"for_statement",
    c"while_statement",
    c"do_statement",
    c"switch_statement",
    c"case_statement",
]);
static C_VAR_TYPES: Kinds<1> = kinds([c"declaration"]);
static C_ASSIGN_TYPES: Kinds<2> =
    kinds([c"assignment_expression", c"update_expression"]);

// ==================== BASH ====================
static BASH_FUNC_TYPES: Kinds<1> = kinds([c"function_definition"]);
static BASH_MODULE_TYPES: Kinds<1> = kinds([c"program"]);
static BASH_CALL_TYPES: Kinds<1> = kinds([c"command"]);
static BASH_IMPORT_TYPES: Kinds<1> = kinds([c"command"]);
static BASH_BRANCH_TYPES: Kinds<5> = kinds([
    c"if_statement",
    c"while_statement",
    c"for_statement",
    c"case_statement",
    c"elif_clause",
]);
static BASH_VAR_TYPES: Kinds<1> = kinds([c"variable_assignment"]);

// ==================== ZIG ====================
static ZIG_FUNC_TYPES: Kinds<3> = kinds([
    c"function_declaration",
    c"test_declaration",
    c"function_signature",
]);
static ZIG_CLASS_TYPES: Kinds<3> = kinds([
    c"struct_declaration",
    c"enum_declaration",
    c"union_declaration",
]);
static ZIG_FIELD_TYPES: Kinds<1> = kinds([c"container_field"]);
static ZIG_MODULE_TYPES: Kinds<1> = kinds([c"source_file"]);
static ZIG_CALL_TYPES: Kinds<2> =
    kinds([c"call_expression", c"builtin_function"]);
static ZIG_IMPORT_TYPES: Kinds<1> = kinds([c"builtin_function"]);
static ZIG_BRANCH_TYPES: Kinds<4> = kinds([
    c"if_statement",
    c"for_statement",
    c"while_statement",
    c"switch_expression",
]);
static ZIG_VAR_TYPES: Kinds<1> = kinds([c"variable_declaration"]);
static ZIG_ASSIGN_TYPES: Kinds<1> = kinds([c"assignment_expression"]);

// ==================== ELIXIR ====================
static ELIXIR_FUNC_TYPES: Kinds<2> = kinds([c"call", c"anonymous_function"]);
static ELIXIR_MODULE_TYPES: Kinds<1> = kinds([c"source"]);
static ELIXIR_CALL_TYPES: Kinds<2> = kinds([c"call", c"binary_operator"]);
static ELIXIR_IMPORT_TYPES: Kinds<1> = kinds([c"call"]);
static ELIXIR_BRANCH_TYPES: Kinds<1> = kinds([c"call"]);
static ELIXIR_VAR_TYPES: Kinds<1> = kinds([c"binary_operator"]);

// ==================== HASKELL ====================
// "bind" = a nullary value binding (`foo = 1`); has a `name` field like `function`.
// `signature` (type annotations) is suppressed in cbm_resolve_func_name so it never doubles.
static HASKELL_FUNC_TYPES: Kinds<3> =
    kinds([c"function", c"signature", c"bind"]);
static HASKELL_CLASS_TYPES: Kinds<3> =
    kinds([c"class", c"data_type", c"newtype"]);
static HASKELL_MODULE_TYPES: Kinds<1> = kinds([c"haskell"]);
static HASKELL_CALL_TYPES: Kinds<2> = kinds([c"infix", c"apply"]);
static HASKELL_IMPORT_TYPES: Kinds<2> = kinds([c"import", c"instance"]);
static HASKELL_BRANCH_TYPES: Kinds<6> =
    kinds([c"match", c"guards", c"if", c"case", c"do", c"boolean"]);
static HASKELL_VAR_TYPES: Kinds<1> = kinds([c"function"]);

// ==================== OCAML ====================
static OCAML_FUNC_TYPES: Kinds<3> = kinds([
    c"value_definition",
    c"constructor_declaration",
    c"method_definition",
]);
static OCAML_CLASS_TYPES: Kinds<5> = kinds([
    c"type_definition",
    c"class_definition",
    c"module_definition",
    c"exception_definition",
    c"record_declaration",
]);
static OCAML_MODULE_TYPES: Kinds<1> = kinds([c"compilation_unit"]);
static OCAML_CALL_TYPES: Kinds<5> = kinds([
    c"application_expression",
    c"infix_expression",
    c"method_invocation",
    c"module_application",
    c"new_expression",
]);
static OCAML_IMPORT_TYPES: Kinds<2> = kinds([c"open_module", c"include"]);
static OCAML_BRANCH_TYPES: Kinds<3> =
    kinds([c"match_expression", c"if_expression", c"match_case"]);
static OCAML_VAR_TYPES: Kinds<1> = kinds([c"value_definition"]);

// ==================== OBJECTIVE-C ====================
static OBJC_FUNC_TYPES: Kinds<3> = kinds([
    c"function_definition",
    c"method_definition",
    c"method_declaration",
]);
static OBJC_CLASS_TYPES: Kinds<9> = kinds([
    c"class_interface",
    c"class_implementation",
    c"protocol_declaration",
    c"class_declaration",
    c"enum_specifier",
    c"struct_declaration",
    c"struct_specifier",
    c"type_definition",
    c"union_specifier",
]);
static OBJC_FIELD_TYPES: Kinds<1> = kinds([c"property_declaration"]);
static OBJC_MODULE_TYPES: Kinds<1> = kinds([c"translation_unit"]);
static OBJC_CALL_TYPES: Kinds<2> =
    kinds([c"call_expression", c"message_expression"]);
static OBJC_IMPORT_TYPES: Kinds<2> =
    kinds([c"preproc_import", c"preproc_include"]);
static OBJC_BRANCH_TYPES: Kinds<4> = kinds([
    c"if_statement",
    c"for_statement",
    c"while_statement",
    c"switch_statement",
]);
static OBJC_VAR_TYPES: Kinds<1> = kinds([c"declaration"]);
static OBJC_ASSIGN_TYPES: Kinds<1> = kinds([c"assignment_expression"]);

// ==================== SWIFT ====================
// protocol_function_declaration: a protocol's method requirements. Needed here as
// well as in the name resolvers — extract_class_methods gates on this set, so
// without the entry the requirement is walked and then discarded.
static SWIFT_FUNC_TYPES: Kinds<5> = kinds([
    c"function_declaration",
    c"protocol_function_declaration",
    c"macro_declaration",
    c"init_declaration",
    c"deinit_declaration",
]);

// KNOWN GAP: struct_declaration and enum_declaration are not node types in the
// vendored tree-sitter-swift grammar — it models both as class_declaration — so
// these two entries never match anything, and a bare Swift `enum` is labeled
// Class rather than Enum. Left in place deliberately: they are the only marker
// of that modelling gap, and deleting them would hide it. Tracked separately.
static SWIFT_CLASS_TYPES: Kinds<4> = kinds([
    c"class_declaration",
    c"protocol_declaration",
    c"struct_declaration",
    c"enum_declaration",
]);
static SWIFT_FIELD_TYPES: Kinds<1> = kinds([c"property_declaration"]);
static SWIFT_MODULE_TYPES: Kinds<1> = kinds([c"source_file"]);
static SWIFT_CALL_TYPES: Kinds<3> = kinds([
    c"call_expression",
    c"constructor_expression",
    c"macro_invocation",
]);
static SWIFT_IMPORT_TYPES: Kinds<2> = kinds([c"import_declaration", c"import"]);
static SWIFT_BRANCH_TYPES: Kinds<5> = kinds([
    c"if_statement",
    c"guard_statement",
    c"for_statement",
    c"while_statement",
    c"switch_statement",
]);
static SWIFT_VAR_TYPES: Kinds<1> = kinds([c"property_declaration"]);
static SWIFT_ASSIGN_TYPES: Kinds<1> = kinds([c"assignment"]);
static SWIFT_THROW_TYPES: Kinds<1> = kinds([c"throw_statement"]);
static SWIFT_DECORATOR_TYPES: Kinds<1> = kinds([c"attribute"]);

// ==================== DART ====================
static DART_FUNC_TYPES: Kinds<3> = kinds([
    c"function_signature",
    c"method_signature",
    c"lambda_expression",
]);
static DART_CLASS_TYPES: Kinds<5> = kinds([
    c"class_definition",
    c"enum_declaration",
    c"mixin_declaration",
    c"extension_declaration",
    c"type_alias",
]);
static DART_FIELD_TYPES: Kinds<1> = kinds([c"declaration"]);
static DART_MODULE_TYPES: Kinds<1> = kinds([c"program"]);
static DART_CALL_TYPES: Kinds<2> = kinds([c"selector", c"new_expression"]);
static DART_IMPORT_TYPES: Kinds<3> =
    kinds([c"import_or_export", c"extends", c"import"]);
static DART_BRANCH_TYPES: Kinds<4> = kinds([
    c"if_statement",
    c"for_statement",
    c"while_statement",
    c"switch_statement",
]);
static DART_VAR_TYPES: Kinds<1> = kinds([c"declaration"]);
static DART_ASSIGN_TYPES: Kinds<1> = kinds([c"assignment_expression"]);
static DART_THROW_TYPES: Kinds<1> = kinds([c"throw_expression"]);
static DART_DECORATOR_TYPES: Kinds<1> = kinds([c"annotation"]);

// ==================== PERL ====================
static PERL_FUNC_TYPES: Kinds<1> = kinds([c"subroutine_declaration_statement"]);
static PERL_MODULE_TYPES: Kinds<1> = kinds([c"source_file"]);
static PERL_CALL_TYPES: Kinds<4> = kinds([
    c"ambiguous_function_call_expression",
    c"function_call_expression",
    c"func1op_call_expression",
    c"method_call_expression",
]);
static PERL_IMPORT_TYPES: Kinds<3> =
    kinds([c"use_statement", c"require_statement", c"require"]);
static PERL_BRANCH_TYPES: Kinds<5> = kinds([
    c"if_statement",
    c"unless_statement",
    c"for_statement",
    c"foreach_statement",
    c"while_statement",
]);
static PERL_VAR_TYPES: Kinds<2> =
    kinds([c"variable_declaration", c"expression_statement"]);
static PERL_ASSIGN_TYPES: Kinds<1> = kinds([c"assignment_expression"]);

// ==================== GROOVY ====================
static GROOVY_FUNC_TYPES: Kinds<2> =
    kinds([c"function_definition", c"function_declaration"]);
static GROOVY_CLASS_TYPES: Kinds<1> = kinds([c"class_definition"]);
static GROOVY_MODULE_TYPES: Kinds<1> = kinds([c"source_file"]);
static GROOVY_CALL_TYPES: Kinds<2> =
    kinds([c"function_call", c"juxt_function_call"]);
static GROOVY_IMPORT_TYPES: Kinds<3> =
    kinds([c"groovy_import", c"extends", c"import"]);
static GROOVY_BRANCH_TYPES: Kinds<4> = kinds([
    c"if_statement",
    c"for_statement",
    c"while_statement",
    c"switch_statement",
]);
static GROOVY_VAR_TYPES: Kinds<1> = kinds([c"declaration"]);
static GROOVY_ASSIGN_TYPES: Kinds<1> = kinds([c"assignment"]);
static GROOVY_THROW_TYPES: Kinds<1> = kinds([c"throw_statement"]);
static GROOVY_DECORATOR_TYPES: Kinds<1> = kinds([c"annotation"]);

// ==================== ERLANG ====================
static ERLANG_FUNC_TYPES: Kinds<1> = kinds([c"function_clause"]);
static ERLANG_CLASS_TYPES: Kinds<1> = kinds([c"type_alias"]);
static ERLANG_MODULE_TYPES: Kinds<1> = kinds([c"source_file"]);
static ERLANG_CALL_TYPES: Kinds<1> = kinds([c"call"]);
static ERLANG_IMPORT_TYPES: Kinds<3> =
    kinds([c"module_attribute", c"import", c"include"]);
static ERLANG_BRANCH_TYPES: Kinds<3> =
    kinds([c"if_expression", c"case_expression", c"receive_expression"]);
static ERLANG_VAR_TYPES: Kinds<2> = kinds([c"pp_define", c"record_decl"]);
static ERLANG_ASSIGN_TYPES: Kinds<1> = kinds([c"match_expression"]);
static ERLANG_THROW_TYPES: Kinds<1> = kinds([c"call"]);

// ==================== R ====================
static R_FUNC_TYPES: Kinds<1> = kinds([c"function_definition"]);
static R_MODULE_TYPES: Kinds<1> = kinds([c"program"]);
static R_CALL_TYPES: Kinds<1> = kinds([c"call"]);
static R_IMPORT_TYPES: Kinds<1> = kinds([c"call"]);
static R_BRANCH_TYPES: Kinds<3> =
    kinds([c"if_statement", c"for_statement", c"while_statement"]);
static R_VAR_TYPES: Kinds<1> = kinds([c"binary_operator"]);

// ==================== HTML ====================
static HTML_MODULE_TYPES: Kinds<1> = kinds([c"document"]);

// ==================== CSS ====================
static CSS_CALL_TYPES: Kinds<1> = kinds([c"call_expression"]);
static CSS_MODULE_TYPES: Kinds<1> = kinds([c"stylesheet"]);
static CSS_IMPORT_TYPES: Kinds<1> = kinds([c"import_statement"]);

// ==================== SCSS ====================
static SCSS_FUNC_TYPES: Kinds<2> =
    kinds([c"mixin_statement", c"function_statement"]);
static SCSS_MODULE_TYPES: Kinds<1> = kinds([c"stylesheet"]);
static SCSS_CALL_TYPES: Kinds<2> =
    kinds([c"call_expression", c"include_statement"]);
static SCSS_IMPORT_TYPES: Kinds<3> =
    kinds([c"import_statement", c"use_statement", c"include_statement"]);
static SCSS_BRANCH_TYPES: Kinds<1> = kinds([c"if_statement"]);
static SCSS_VAR_TYPES: Kinds<1> = kinds([c"declaration"]);

// ==================== YAML ====================
static YAML_MODULE_TYPES: Kinds<1> = kinds([c"stream"]);
static YAML_VAR_TYPES: Kinds<1> = kinds([c"block_mapping_pair"]);

// ==================== TOML ====================
static TOML_MODULE_TYPES: Kinds<1> = kinds([c"document"]);
static TOML_CLASS_TYPES: Kinds<2> = kinds([c"table", c"table_array_element"]);
static TOML_VAR_TYPES: Kinds<1> = kinds([c"pair"]);

// ==================== HCL ====================
static HCL_CLASS_TYPES: Kinds<1> = kinds([c"block"]);
static HCL_MODULE_TYPES: Kinds<1> = kinds([c"config_file"]);
static HCL_CALL_TYPES: Kinds<1> = kinds([c"function_call"]);
static HCL_VAR_TYPES: Kinds<1> = kinds([c"attribute"]);

// ==================== SQL ====================
static SQL_FUNC_TYPES: Kinds<3> = kinds([
    c"create_function",
    c"function_declaration",
    c"create_procedure",
]);
static SQL_FIELD_TYPES: Kinds<1> = kinds([c"column_definition"]);

// create_table/create_view route through the class-def path where
// extract_sql_ddl_class_def turns them into first-class Table/View nodes
// (previously they were generic Variable nodes via sql_var_types).
static SQL_CLASS_TYPES: Kinds<4> = kinds([
    c"custom_type",
    c"create_table",
    c"create_view",
    c"create_materialized_view",
]);
static SQL_MODULE_TYPES: Kinds<1> = kinds([c"program"]);
static SQL_CALL_TYPES: Kinds<1> = kinds([c"invocation"]);
static SQL_BRANCH_TYPES: Kinds<2> = kinds([c"if_statement", c"case_expression"]);

// ==================== DOCKERFILE ====================
static DOCKERFILE_MODULE_TYPES: Kinds<1> = kinds([c"source_file"]);
static DOCKERFILE_VAR_TYPES: Kinds<2> =
    kinds([c"env_instruction", c"arg_instruction"]);

// ==================== ENV ACCESS ====================
static GO_ENV_FUNCS: Kinds<2> = kinds([c"os.Getenv", c"os.LookupEnv"]);
static PY_ENV_FUNCS: Kinds<2> = kinds([c"os.getenv", c"os.environ.get"]);
static PY_ENV_MEMBERS: Kinds<1> = kinds([c"os.environ"]);
static JS_ENV_MEMBERS: Kinds<1> = kinds([c"process.env"]);
static TS_ENV_MEMBERS: Kinds<1> = kinds([c"process.env"]);
static RUST_ENV_FUNCS: Kinds<2> = kinds([c"env::var", c"std::env::var"]);
static JAVA_ENV_FUNCS: Kinds<2> =
    kinds([c"System.getenv", c"System.getProperty"]);
static CPP_ENV_FUNCS: Kinds<2> = kinds([c"getenv", c"std::getenv"]);
static CS_ENV_FUNCS: Kinds<1> = kinds([c"Environment.GetEnvironmentVariable"]);
static PHP_ENV_FUNCS: Kinds<2> = kinds([c"getenv", c"env"]);
static LUA_ENV_FUNCS: Kinds<1> = kinds([c"os.getenv"]);
static SCALA_ENV_FUNCS: Kinds<3> =
    kinds([c"sys.env", c"System.getenv", c"System.getProperty"]);
static KOTLIN_ENV_FUNCS: Kinds<2> =
    kinds([c"System.getenv", c"System.getProperty"]);
static RUBY_ENV_MEMBERS: Kinds<1> = kinds([c"ENV"]);
static C_ENV_FUNCS: Kinds<1> = kinds([c"getenv"]);
static ZIG_ENV_FUNCS: Kinds<1> = kinds([c"std.os.getenv"]);
static ELIXIR_ENV_FUNCS: Kinds<1> = kinds([c"System.get_env"]);
static HASKELL_ENV_FUNCS: Kinds<2> = kinds([c"lookupEnv", c"getEnv"]);
static OCAML_ENV_FUNCS: Kinds<1> = kinds([c"Sys.getenv"]);
static R_ENV_FUNCS: Kinds<1> = kinds([c"Sys.getenv"]);
static PERL_ENV_FUNCS: Kinds<1> = kinds([c"$ENV"]);

// ==================== CLOJURE ====================
// Clojure def-forms (defn/def/...) are `list_lit` nodes; gating the actual
// def-vs-call distinction happens in cbm_resolve_func_name (returns NULL for a
// non-def list_lit such as a call), so non-def lists never push a SCOPE_FUNC.
static CLOJURE_FUNC_TYPES: Kinds<1> = kinds([c"list_lit"]);
static CLOJURE_MODULE_TYPES: Kinds<1> = kinds([c"source"]);
static CLOJURE_CALL_TYPES: Kinds<1> = kinds([c"list_lit"]);

// ==================== F# ====================
// Top-level `let f () = ...` parses to function_or_value_defn (module-level
// value_declaration is aliased to declaration_expression, which wraps it). The
// name lives on a function_declaration_left/value_declaration_left child — see
// the CBM_LANG_FSHARP branch in cbm_resolve_func_name.
static FSHARP_FUNC_TYPES: Kinds<3> = kinds([
    c"function_declaration",
    c"value_declaration",
    c"function_or_value_defn",
]);
static FSHARP_CLASS_TYPES: Kinds<2> =
    kinds([c"type_definition", c"exception_definition"]);
static FSHARP_MODULE_TYPES: Kinds<1> = kinds([c"file"]);
static FSHARP_CALL_TYPES: Kinds<1> = kinds([c"application_expression"]);
static FSHARP_IMPORT_TYPES: Kinds<3> =
    kinds([c"import_decl", c"open_expression", c"instance"]);
static FSHARP_BRANCH_TYPES: Kinds<5> = kinds([
    c"if_expression",
    c"for_expression",
    c"while_expression",
    c"match_expression",
    c"elif_expression",
]);
static FSHARP_VAR_TYPES: Kinds<1> = kinds([c"value_declaration"]);

// ==================== JULIA ====================
// `assignment` covers Julia short-form `f(x) = body` (the grammar parses it as an
// assignment with a call_expression LHS, not a short_function_definition). The
// resolver names it only when the LHS is a call, so plain `x = 5` is not a def.
static JULIA_FUNC_TYPES: Kinds<3> = kinds([
    c"function_definition",
    c"short_function_definition",
    c"assignment",
]);
static JULIA_CLASS_TYPES: Kinds<3> = kinds([
    c"struct_definition",
    c"abstract_definition",
    c"primitive_definition",
]);
static JULIA_MODULE_TYPES: Kinds<1> = kinds([c"source_file"]);
static JULIA_CALL_TYPES: Kinds<2> =
    kinds([c"call_expression", c"broadcast_call_expression"]);
static JULIA_IMPORT_TYPES: Kinds<4> = kinds([
    c"import_statement",
    c"using_statement",
    c"export_statement",
    c"import",
]);
static JULIA_BRANCH_TYPES: Kinds<4> = kinds([
    c"if_statement",
    c"for_statement",
    c"while_statement",
    c"try_statement",
]);
static JULIA_VAR_TYPES: Kinds<2> = kinds([c"const_statement", c"assignment"]);
static JULIA_ASSIGN_TYPES: Kinds<2> =
    kinds([c"assignment", c"compound_assignment_expression"]);
static JULIA_THROW_TYPES: Kinds<1> = kinds([c"throw_statement"]);

// ==================== VIM SCRIPT ====================
static VIM_FUNC_TYPES: Kinds<3> = kinds([
    c"function_definition",
    c"function_declaration",
    c"lambda_expression",
]);
static VIM_IMPORT_TYPES: Kinds<1> = kinds([c"include"]);
static VIM_MODULE_TYPES: Kinds<1> = kinds([c"script_file"]);
static VIM_CALL_TYPES: Kinds<3> =
    kinds([c"call_expression", c"call", c"command"]);
static VIM_BRANCH_TYPES: Kinds<4> = kinds([
    c"if_statement",
    c"for_statement",
    c"while_statement",
    c"try_statement",
]);
static VIM_VAR_TYPES: Kinds<1> = kinds([c"let_statement"]);

// ==================== NIX ====================
static NIX_FUNC_TYPES: Kinds<1> = kinds([c"function_expression"]);
static NIX_MODULE_TYPES: Kinds<1> = kinds([c"source_expression"]);
static NIX_CALL_TYPES: Kinds<1> = kinds([c"apply_expression"]);
static NIX_BRANCH_TYPES: Kinds<1> = kinds([c"if_expression"]);
static NIX_VAR_TYPES: Kinds<1> = kinds([c"binding"]);

// ==================== COMMON LISP ====================
static COMMONLISP_FUNC_TYPES: Kinds<1> = kinds([c"defun"]);
static COMMONLISP_IMPORT_TYPES: Kinds<1> = kinds([c"with_clause"]);
static COMMONLISP_MODULE_TYPES: Kinds<1> = kinds([c"source"]);
static COMMONLISP_CALL_TYPES: Kinds<1> = kinds([c"list_lit"]);

// ==================== ELM ====================
static ELM_FUNC_TYPES: Kinds<2> =
    kinds([c"value_declaration", c"function_declaration"]);
static ELM_CLASS_TYPES: Kinds<3> = kinds([
    c"type_declaration",
    c"type_alias_declaration",
    c"module_declaration",
]);
static ELM_MODULE_TYPES: Kinds<1> = kinds([c"file"]);
static ELM_CALL_TYPES: Kinds<1> = kinds([c"function_call_expr"]);
static ELM_IMPORT_TYPES: Kinds<1> = kinds([c"import"]);
static ELM_BRANCH_TYPES: Kinds<2> = kinds([c"case_of_expr", c"if_else_expr"]);

// ==================== FORTRAN ====================
// subroutine/function wrap an inner *_statement that carries the `name` field;
// function_statement was already present, subroutine_statement was missing.
static FORTRAN_FUNC_TYPES: Kinds<4> = kinds([
    c"function",
    c"subroutine",
    c"function_statement",
    c"subroutine_statement",
]);
static FORTRAN_CLASS_TYPES: Kinds<2> =
    kinds([c"derived_type_definition", c"enum_statement"]);
static FORTRAN_MODULE_TYPES: Kinds<1> = kinds([c"translation_unit"]);
static FORTRAN_CALL_TYPES: Kinds<2> =
    kinds([c"call_expression", c"subroutine_call"]);
static FORTRAN_IMPORT_TYPES: Kinds<8> = kinds([
    c"use_statement",
    c"include_statement",
    c"extends",
    c"import",
    c"import_statement",
    c"include",
    c"open_statement",
    c"preproc_include",
]);
static FORTRAN_BRANCH_TYPES: Kinds<4> = kinds([
    c"if_statement",
    c"do_loop_statement",
    c"where_statement",
    c"select_case_statement",
]);
static FORTRAN_VAR_TYPES: Kinds<1> = kinds([c"variable_declaration"]);
static FORTRAN_ASSIGN_TYPES: Kinds<1> = kinds([c"assignment_statement"]);

// ==================== CUDA ====================
// CUDA extends C++, reuse cpp types (same grammar family)
// ==================== COBOL ====================
static COBOL_FUNC_TYPES: Kinds<1> = kinds([c"program_definition"]);
static COBOL_IMPORT_TYPES: Kinds<3> =
    kinds([c"open_statement", c"use_statement", c"with_clause"]);
static COBOL_MODULE_TYPES: Kinds<1> = kinds([c"source_file"]);
static COBOL_CALL_TYPES: Kinds<1> = kinds([c"call_statement"]);
static COBOL_BRANCH_TYPES: Kinds<3> =
    kinds([c"if_statement", c"evaluate_statement", c"perform_statement"]);
static COBOL_VAR_TYPES: Kinds<1> = kinds([c"data_description_entry"]);

// ==================== VERILOG ====================
static VERILOG_FUNC_TYPES: Kinds<4> = kinds([
    c"function_declaration",
    c"task_declaration",
    c"function_body_declaration",
    c"function_statement",
]);
static VERILOG_CLASS_TYPES: Kinds<5> = kinds([
    c"module_declaration",
    c"class_declaration",
    c"interface_declaration",
    c"package_declaration",
    c"type_declaration",
]);
static VERILOG_IMPORT_TYPES: Kinds<3> =
    kinds([c"extends", c"import", c"package_import_declaration"]);
static VERILOG_MODULE_TYPES: Kinds<1> = kinds([c"source_file"]);
static VERILOG_CALL_TYPES: Kinds<4> = kinds([
    c"system_tf_call",
    c"subroutine_call",
    c"function_subroutine_call",
    c"method_call",
]);
static VERILOG_BRANCH_TYPES: Kinds<3> = kinds([
    c"conditional_statement",
    c"case_statement",
    c"loop_statement",
]);
static VERILOG_VAR_TYPES: Kinds<2> =
    kinds([c"net_declaration", c"data_declaration"]);
static VERILOG_ASSIGN_TYPES: Kinds<2> =
    kinds([c"blocking_assignment", c"nonblocking_assignment"]);

// ==================== EMACS LISP ====================
static ELISP_FUNC_TYPES: Kinds<2> =
    kinds([c"function_definition", c"macro_definition"]);
static ELISP_MODULE_TYPES: Kinds<1> = kinds([c"source_file"]);
static ELISP_CALL_TYPES: Kinds<1> = kinds([c"list"]);

// ==================== JSON ====================
static JSON_MODULE_TYPES: Kinds<1> = kinds([c"document"]);
static JSON_VAR_TYPES: Kinds<1> = kinds([c"pair"]);

// ==================== XML ====================
static XML_MODULE_TYPES: Kinds<1> = kinds([c"document"]);
static XML_CLASS_TYPES: Kinds<1> = kinds([c"element"]);

// ==================== MARKDOWN ====================
static MARKDOWN_MODULE_TYPES: Kinds<1> = kinds([c"document"]);
static MARKDOWN_CLASS_TYPES: Kinds<2> =
    kinds([c"atx_heading", c"setext_heading"]);

// ==================== MAKEFILE ====================
static MAKEFILE_FUNC_TYPES: Kinds<2> = kinds([c"rule", c"recipe"]);
static MAKEFILE_MODULE_TYPES: Kinds<1> = kinds([c"makefile"]);
static MAKEFILE_CALL_TYPES: Kinds<3> =
    kinds([c"function_call", c"call", c"shell_function"]);
static MAKEFILE_IMPORT_TYPES: Kinds<2> =
    kinds([c"include_directive", c"include"]);
static MAKEFILE_VAR_TYPES: Kinds<1> = kinds([c"variable_assignment"]);

// ==================== CMAKE ====================
static CMAKE_FUNC_TYPES: Kinds<2> = kinds([c"function_def", c"macro_def"]);
static CMAKE_MODULE_TYPES: Kinds<1> = kinds([c"source_file"]);
static CMAKE_CALL_TYPES: Kinds<1> = kinds([c"normal_command"]);

// ==================== PROTOBUF ====================
static PROTOBUF_CLASS_TYPES: Kinds<3> = kinds([c"message", c"enum", c"service"]);
static PROTOBUF_FUNC_TYPES: Kinds<1> = kinds([c"rpc"]);
static PROTOBUF_MODULE_TYPES: Kinds<1> = kinds([c"source_file"]);
static PROTOBUF_FIELD_TYPES: Kinds<3> =
    kinds([c"field", c"map_field", c"oneof_field"]);
static PROTOBUF_IMPORT_TYPES: Kinds<1> = kinds([c"import"]);

// ==================== GRAPHQL ====================
static GRAPHQL_CLASS_TYPES: Kinds<8> = kinds([
    c"object_type_definition",
    c"input_object_type_definition",
    c"enum_type_definition",
    c"interface_type_definition",
    c"union_type_definition",
    c"scalar_type_definition",
    c"type_definition",
    c"object_type_extension",
]);
static GRAPHQL_MODULE_TYPES: Kinds<1> = kinds([c"document"]);
static GRAPHQL_FIELD_TYPES: Kinds<2> =
    kinds([c"field_definition", c"input_value_definition"]);

// ==================== Embedded sub-languages ====================
// Host grammars (Svelte/Vue/HTML/Astro) treat <script> bodies as raw_text and
// do not recurse into them. Declaring the host's script-content node here lets
// the generic embedded walker re-parse that slice with the declared grammar so
// its definitions, imports and calls extract in host-file coordinates. The
// declared language is the default; a <script lang=> attribute overrides it.
// Terminator: an entry whose script_node_type is NULL.

// Tag-dialect CFML keeps <cfscript> bodies as opaque cf_script_content;
// re-parse them with the cfscript grammar through the shared included-
// ranges machinery so defs (and calls) inside legacy components extract
// with absolute coordinates. Distilled from #1412.
static CFML_EMBEDDED_IMPORTS: [CbmEmbeddedLangSpec; 2] = [
    embedded(c"cf_script_tag", c"cf_script_content", lang::CFSCRIPT),
    EMBEDDED_END,
];

static VUE_EMBEDDED_IMPORTS: [CbmEmbeddedLangSpec; 2] = [
    embedded(c"script_element", c"raw_text", lang::JAVASCRIPT),
    EMBEDDED_END,
];

static SVELTE_EMBEDDED_IMPORTS: [CbmEmbeddedLangSpec; 2] = [
    embedded(c"script_element", c"raw_text", lang::JAVASCRIPT),
    EMBEDDED_END,
];

static HTML_EMBEDDED_IMPORTS: [CbmEmbeddedLangSpec; 2] = [
    embedded(c"script_element", c"raw_text", lang::JAVASCRIPT),
    EMBEDDED_END,
];

// Astro component scripts live in the `---` frontmatter fence, which the
// grammar keeps as an unparsed frontmatter_js_block. Astro type-checks the
// fence and its <script> bodies as TypeScript without any lang= attribute
// saying so (the fence cannot carry one), so TypeScript is the deliberate
// default for both; it parses untyped JavaScript unchanged.
static ASTRO_EMBEDDED_IMPORTS: [CbmEmbeddedLangSpec; 3] = [
    embedded(c"frontmatter", c"frontmatter_js_block", lang::TYPESCRIPT),
    embedded(c"script_element", c"raw_text", lang::TYPESCRIPT),
    EMBEDDED_END,
];

// ==================== VUE ====================
static VUE_MODULE_TYPES: Kinds<1> = kinds([c"document"]);

// ==================== SVELTE ====================
static SVELTE_MODULE_TYPES: Kinds<1> = kinds([c"document"]);
static SVELTE_BRANCH_TYPES: Kinds<3> =
    kinds([c"if_statement", c"each_statement", c"await_statement"]);

// ==================== MESON ====================
static MESON_FUNC_TYPES: Kinds<1> = kinds([c"function_expression"]);
static MESON_MODULE_TYPES: Kinds<1> = kinds([c"source_file"]);
static MESON_CALL_TYPES: Kinds<1> = kinds([c"normal_command"]);
static MESON_BRANCH_TYPES: Kinds<2> =
    kinds([c"if_statement", c"foreach_statement"]);
static MESON_VAR_TYPES: Kinds<1> = kinds([c"assignment_statement"]);

// ==================== GLSL ====================
// GLSL extends C, reuse c types (same grammar family)
// ==================== INI ====================
static INI_MODULE_TYPES: Kinds<1> = kinds([c"document"]);
static INI_CLASS_TYPES: Kinds<1> = kinds([c"section"]);
static INI_VAR_TYPES: Kinds<1> = kinds([c"setting"]);

// ==================== MATLAB ====================
static MATLAB_FUNC_TYPES: Kinds<2> =
    kinds([c"function_definition", c"function_signature"]);
static MATLAB_CLASS_TYPES: Kinds<1> = kinds([c"class_definition"]);
static MATLAB_MODULE_TYPES: Kinds<1> = kinds([c"source_file"]);
static MATLAB_CALL_TYPES: Kinds<2> = kinds([c"function_call", c"command"]);
static MATLAB_BRANCH_TYPES: Kinds<5> = kinds([
    c"if_statement",
    c"for_statement",
    c"while_statement",
    c"switch_statement",
    c"try_statement",
]);
static MATLAB_VAR_TYPES: Kinds<1> = kinds([c"assignment"]);

// ==================== LEAN ====================
static LEAN_FUNC_TYPES: Kinds<4> =
    kinds([c"def", c"theorem", c"instance", c"abbrev"]);
static LEAN_CLASS_TYPES: Kinds<3> =
    kinds([c"structure", c"class_inductive", c"inductive"]);
static LEAN_MODULE_TYPES: Kinds<1> = kinds([c"module"]);
static LEAN_CALL_TYPES: Kinds<1> = kinds([c"apply"]);
static LEAN_IMPORT_TYPES: Kinds<3> = kinds([c"import", c"extends", c"instance"]);
static LEAN_BRANCH_TYPES: Kinds<3> = kinds([c"if", c"match", c"do"]);

// ==================== FORM ====================
static FORM_FUNC_TYPES: Kinds<1> = kinds([c"procedure_definition"]);
static FORM_MODULE_TYPES: Kinds<1> = kinds([c"source_file"]);
static FORM_CALL_TYPES: Kinds<1> = kinds([c"call_statement"]);
static FORM_IMPORT_TYPES: Kinds<1> = kinds([c"include_directive"]);
static FORM_BRANCH_TYPES: Kinds<3> =
    kinds([c"if_statement", c"repeat_statement", c"do_loop"]);
static FORM_VAR_TYPES: Kinds<1> = kinds([c"declaration_statement"]);
static FORM_ASSIGN_TYPES: Kinds<1> = kinds([c"substitution_statement"]);

// ==================== MAGMA ====================
static MAGMA_FUNC_TYPES: Kinds<4> = kinds([
    c"function_definition",
    c"procedure_definition",
    c"intrinsic_definition",
    c"anonymous_function",
]);
static MAGMA_MODULE_TYPES: Kinds<1> = kinds([c"source_file"]);
static MAGMA_CALL_TYPES: Kinds<1> = kinds([c"call_expression"]);
static MAGMA_IMPORT_TYPES: Kinds<3> =
    kinds([c"load_statement", c"require", c"require_statement"]);
static MAGMA_BRANCH_TYPES: Kinds<5> = kinds([
    c"if_statement",
    c"for_statement",
    c"while_statement",
    c"repeat_statement",
    c"case_statement",
]);
static MAGMA_VAR_TYPES: Kinds<1> = kinds([c"assignment_statement"]);

// ==================== WOLFRAM ====================
static WOLFRAM_FUNC_TYPES: Kinds<4> =
    kinds([c"set_delayed_top", c"set_top", c"set_delayed", c"set"]);
static WOLFRAM_MODULE_TYPES: Kinds<1> = kinds([c"source_file"]);
static WOLFRAM_CALL_TYPES: Kinds<1> = kinds([c"apply"]);
static WOLFRAM_IMPORT_TYPES: Kinds<1> = kinds([c"get_top"]);

// ==================== NEW LANG ENV ACCESS ====================
static JULIA_ENV_FUNCS: Kinds<1> = kinds([c"ENV"]);
static NIX_ENV_FUNCS: Kinds<1> = kinds([c"builtins.getEnv"]);
static FORTRAN_ENV_FUNCS: Kinds<1> = kinds([c"get_environment_variable"]);
static FSHARP_ENV_FUNCS: Kinds<1> =
    kinds([c"Environment.GetEnvironmentVariable"]);

// ==================== D ====================
static D_FUNC_TYPES: Kinds<3> =
    kinds([c"function_declaration", c"constructor", c"destructor"]);
static D_CLASS_TYPES: Kinds<7> = kinds([
    c"class_declaration",
    c"struct_declaration",
    c"interface_declaration",
    c"union_declaration",
    c"enum_declaration",
    c"module_declaration",
    c"module_def",
]);
static D_FIELD_TYPES: Kinds<1> = kinds([c"variable_declaration"]);
static D_CALL_TYPES: Kinds<2> = kinds([c"call_expression", c"new_expression"]);
static D_IMPORT_TYPES: Kinds<2> = kinds([c"import_declaration", c"import"]);
static D_BRANCH_TYPES: Kinds<8> = kinds([
    c"if_statement",
    c"for_statement",
    c"foreach_statement",
    c"while_statement",
    c"do_statement",
    c"switch_statement",
    c"try_statement",
    c"catch_statement",
]);
static D_VAR_TYPES: Kinds<2> =
    kinds([c"auto_declaration", c"variable_declaration"]);
static D_ASSIGN_TYPES: Kinds<1> = kinds([c"assignment_expression"]);
static D_THROW_TYPES: Kinds<1> = kinds([c"throw_expression"]);

// ==================== LLVM IR ====================
static LLVM_FUNC_TYPES: Kinds<1> = kinds([c"function_header"]);
static LLVM_CALL_TYPES: Kinds<3> =
    kinds([c"call", c"invoke", c"instruction_call"]);
static LLVM_BRANCH_TYPES: Kinds<2> = kinds([c"br", c"switch"]);
static LLVM_VAR_TYPES: Kinds<2> = kinds([c"local_var", c"global_var"]);

// ==================== NEW LANGUAGE MODULE TYPES ====================
static SOLIDITY_FUNC_TYPES: Kinds<4> = kinds([
    c"function_definition",
    c"constructor_definition",
    c"modifier_definition",
    c"fallback_receive_definition",
]);
static SOLIDITY_CLASS_TYPES: Kinds<6> = kinds([
    c"contract_declaration",
    c"interface_declaration",
    c"library_declaration",
    c"struct_declaration",
    c"enum_declaration",
    c"type_alias",
]);
static SOLIDITY_FIELD_TYPES: Kinds<2> =
    kinds([c"state_variable_declaration", c"struct_member"]);
static SOLIDITY_CALL_TYPES: Kinds<3> =
    kinds([c"call_expression", c"call", c"new_expression"]);
static SOLIDITY_IMPORT_TYPES: Kinds<3> =
    kinds([c"import_directive", c"import", c"using_directive"]);
static SOLIDITY_DECORATOR_TYPES: Kinds<1> = kinds([c"modifier_invocation"]);
static SOLIDITY_BRANCH_TYPES: Kinds<5> = kinds([
    c"if_statement",
    c"for_statement",
    c"while_statement",
    c"do_while_statement",
    c"try_statement",
]);
static SOLIDITY_VAR_TYPES: Kinds<2> = kinds([
    c"variable_declaration_statement",
    c"state_variable_declaration",
]);
static SOLIDITY_ASSIGN_TYPES: Kinds<2> =
    kinds([c"assignment_expression", c"augmented_assignment_expression"]);
static SOLIDITY_THROW_TYPES: Kinds<2> =
    kinds([c"revert_statement", c"emit_statement"]);
static SOLIDITY_MODULE_TYPES: Kinds<1> = kinds([c"source_file"]);
static TYPST_FUNC_TYPES: Kinds<2> = kinds([c"lambda", c"let"]);
static TYPST_CALL_TYPES: Kinds<1> = kinds([c"call"]);
static TYPST_IMPORT_TYPES: Kinds<2> = kinds([c"import", c"include"]);
static TYPST_BRANCH_TYPES: Kinds<3> = kinds([c"if", c"for", c"while"]);
static TYPST_VAR_TYPES: Kinds<1> = kinds([c"let"]);
static TYPST_ASSIGN_TYPES: Kinds<2> = kinds([c"assign", c"set"]);
static TYPST_MODULE_TYPES: Kinds<1> = kinds([c"source_file"]);
static GDSCRIPT_FUNC_TYPES: Kinds<3> =
    kinds([c"function_definition", c"constructor_definition", c"lambda"]);
static GDSCRIPT_CLASS_TYPES: Kinds<2> =
    kinds([c"class_definition", c"enum_definition"]);
static GDSCRIPT_FIELD_TYPES: Kinds<4> = kinds([
    c"variable_statement",
    c"export_variable_statement",
    c"onready_variable_statement",
    c"signal_statement",
]);
static GDSCRIPT_CALL_TYPES: Kinds<3> =
    kinds([c"call", c"attribute_call", c"base_call"]);
static GDSCRIPT_IMPORT_TYPES: Kinds<3> =
    kinds([c"extends_statement", c"class_name_statement", c"extends"]);
static GDSCRIPT_BRANCH_TYPES: Kinds<4> = kinds([
    c"if_statement",
    c"for_statement",
    c"while_statement",
    c"match_statement",
]);
static GDSCRIPT_VAR_TYPES: Kinds<2> =
    kinds([c"variable_statement", c"const_statement"]);
static GDSCRIPT_ASSIGN_TYPES: Kinds<2> =
    kinds([c"assignment", c"augmented_assignment"]);
static GDSCRIPT_DECORATOR_TYPES: Kinds<1> = kinds([c"annotation"]);
static GDSCRIPT_MODULE_TYPES: Kinds<1> = kinds([c"source_file"]);
static GLEAM_FUNC_TYPES: Kinds<3> =
    kinds([c"function", c"anonymous_function", c"external_function"]);
static GLEAM_CLASS_TYPES: Kinds<3> =
    kinds([c"type_definition", c"type_alias", c"custom_type"]);
static GLEAM_FIELD_TYPES: Kinds<1> = kinds([c"field"]);
static GLEAM_CALL_TYPES: Kinds<1> = kinds([c"function_call"]);
static GLEAM_IMPORT_TYPES: Kinds<2> = kinds([c"import", c"unqualified_import"]);
static GLEAM_BRANCH_TYPES: Kinds<2> = kinds([c"case", c"case_clause"]);
static GLEAM_VAR_TYPES: Kinds<2> = kinds([c"let", c"constant"]);
static GLEAM_ASSIGN_TYPES: Kinds<1> = kinds([c"let_assert"]);
static GLEAM_MODULE_TYPES: Kinds<1> = kinds([c"source_file"]);
static POWERSHELL_FUNC_TYPES: Kinds<1> = kinds([c"function_statement"]);
static POWERSHELL_CLASS_TYPES: Kinds<3> =
    kinds([c"class_statement", c"enum_statement", c"type_spec"]);
static POWERSHELL_CALL_TYPES: Kinds<2> =
    kinds([c"invokation_expression", c"command"]);
static POWERSHELL_IMPORT_TYPES: Kinds<1> = kinds([c"using_statement"]);
static POWERSHELL_BRANCH_TYPES: Kinds<8> = kinds([
    c"if_statement",
    c"for_statement",
    c"foreach_statement",
    c"while_statement",
    c"do_statement",
    c"switch_statement",
    c"try_statement",
    c"catch_clause",
]);
static POWERSHELL_VAR_TYPES: Kinds<1> = kinds([c"variable"]);
static POWERSHELL_ASSIGN_TYPES: Kinds<1> = kinds([c"assignment_expression"]);
static POWERSHELL_THROW_TYPES: Kinds<1> = kinds([c"throw"]);
static POWERSHELL_MODULE_TYPES: Kinds<1> = kinds([c"program"]);
static PASCAL_FUNC_TYPES: Kinds<2> = kinds([c"defProc", c"declProc"]);
static PASCAL_CLASS_TYPES: Kinds<5> = kinds([
    c"declClass",
    c"declIntf",
    c"declHelper",
    c"declObject",
    c"declRecord",
]);
static PASCAL_FIELD_TYPES: Kinds<2> = kinds([c"declField", c"declProp"]);
static PASCAL_CALL_TYPES: Kinds<1> = kinds([c"exprCall"]);
static PASCAL_IMPORT_TYPES: Kinds<1> = kinds([c"declUses"]);
static PASCAL_BRANCH_TYPES: Kinds<8> = kinds([
    c"if", c"ifElse", c"while", c"repeat", c"for", c"foreach", c"try", c"case",
]);
static PASCAL_VAR_TYPES: Kinds<2> = kinds([c"declVar", c"declConst"]);
static PASCAL_ASSIGN_TYPES: Kinds<1> = kinds([c"assignment"]);
static PASCAL_THROW_TYPES: Kinds<1> = kinds([c"raise"]);
static PASCAL_MODULE_TYPES: Kinds<1> = kinds([c"source_file"]);
static D_MODULE_TYPES: Kinds<1> = kinds([c"source_file"]);

// Scheme def-forms (`(define (f ..) ..)`) are `list` nodes; the def-vs-call
// gate is in cbm_resolve_func_name (returns NULL for a non-def list).
static SCHEME_FUNC_TYPES: Kinds<1> = kinds([c"list"]);
static SCHEME_CALL_TYPES: Kinds<1> = kinds([c"list"]);
static SCHEME_VAR_TYPES: Kinds<1> = kinds([c"symbol"]);
static SCHEME_MODULE_TYPES: Kinds<1> = kinds([c"program"]);

// Chialisp: a deliberately generic s-expression grammar (tools/tree-sitter-chialisp).
// Every parenthesized form is a `list` and every atom a `symbol`; which lists are
// definitions is decided in extract_defs.c, not by the parser. Root is `source_file`.
static CHIALISP_FUNC_TYPES: Kinds<1> = kinds([c"list"]);
static CHIALISP_CALL_TYPES: Kinds<1> = kinds([c"list"]);
static CHIALISP_VAR_TYPES: Kinds<1> = kinds([c"symbol"]);
static CHIALISP_MODULE_TYPES: Kinds<1> = kinds([c"source_file"]);
static FENNEL_FUNC_TYPES: Kinds<3> = kinds([c"fn", c"lambda", c"hashfn"]);
static FENNEL_CALL_TYPES: Kinds<1> = kinds([c"list"]);
static FENNEL_BRANCH_TYPES: Kinds<3> = kinds([c"each", c"for", c"match"]);
static FENNEL_VAR_TYPES: Kinds<1> = kinds([c"symbol"]);
static FENNEL_ASSIGN_TYPES: Kinds<5> =
    kinds([c"set", c"var", c"local", c"global", c"let"]);
static FENNEL_MODULE_TYPES: Kinds<1> = kinds([c"program"]);
static FISH_FUNC_TYPES: Kinds<1> = kinds([c"function_definition"]);
static FISH_CALL_TYPES: Kinds<1> = kinds([c"command"]);
static FISH_BRANCH_TYPES: Kinds<4> = kinds([
    c"if_statement",
    c"switch_statement",
    c"while_statement",
    c"for_statement",
]);
static FISH_VAR_TYPES: Kinds<1> = kinds([c"variable"]);
static FISH_MODULE_TYPES: Kinds<1> = kinds([c"program"]);

// Only `func_def` (a named `function f(){}`) is a callable. A `rule` (`{...}` /
// `/re/{...}` / BEGIN/END) is ANONYMOUS top-level executable code — it cannot be
// called by name, so a call inside a rule is legitimately Module-sourced, and a
// rule must NOT be treated as a function boundary.
static AWK_FUNC_TYPES: Kinds<1> = kinds([c"func_def"]);
static AWK_CALL_TYPES: Kinds<1> = kinds([c"func_call"]);
static AWK_BRANCH_TYPES: Kinds<6> = kinds([
    c"if_statement",
    c"for_statement",
    c"for_in_statement",
    c"while_statement",
    c"do_while_statement",
    c"switch_statement",
]);
static AWK_VAR_TYPES: Kinds<1> = kinds([c"identifier"]);
static AWK_ASSIGN_TYPES: Kinds<2> = kinds([c"assignment_exp", c"update_exp"]);
static AWK_MODULE_TYPES: Kinds<1> = kinds([c"program"]);
static ZSH_FUNC_TYPES: Kinds<1> = kinds([c"function_definition"]);
static ZSH_CALL_TYPES: Kinds<2> = kinds([c"command", c"call_expression"]);
static ZSH_BRANCH_TYPES: Kinds<4> = kinds([
    c"if_statement",
    c"case_statement",
    c"while_statement",
    c"for_statement",
]);
static ZSH_VAR_TYPES: Kinds<1> = kinds([c"variable_name"]);
static ZSH_ASSIGN_TYPES: Kinds<1> = kinds([c"variable_assignment"]);
static ZSH_MODULE_TYPES: Kinds<1> = kinds([c"program"]);
static TCL_FUNC_TYPES: Kinds<1> = kinds([c"procedure"]);
static TCL_CLASS_TYPES: Kinds<1> = kinds([c"namespace"]);
static TCL_CALL_TYPES: Kinds<1> = kinds([c"command"]);
static TCL_BRANCH_TYPES: Kinds<5> =
    kinds([c"if", c"while", c"foreach", c"try", c"catch"]);
static TCL_VAR_TYPES: Kinds<1> = kinds([c"variable_substitution"]);
static TCL_MODULE_TYPES: Kinds<1> = kinds([c"source_file"]);
static ADA_FUNC_TYPES: Kinds<4> = kinds([
    c"subprogram_declaration",
    c"subprogram_body",
    c"entry_declaration",
    c"expression_function_declaration",
]);
static ADA_CLASS_TYPES: Kinds<8> = kinds([
    c"type_declaration",
    c"full_type_declaration",
    c"package_declaration",
    c"protected_type_declaration",
    c"task_type_declaration",
    c"component_declaration",
    c"object_declaration",
    c"record_type_definition",
]);
static ADA_FIELD_TYPES: Kinds<1> = kinds([c"component_declaration"]);
static ADA_CALL_TYPES: Kinds<2> =
    kinds([c"function_call", c"procedure_call_statement"]);
static ADA_IMPORT_TYPES: Kinds<2> = kinds([c"with_clause", c"use_clause"]);
static ADA_BRANCH_TYPES: Kinds<6> = kinds([
    c"if_statement",
    c"for_loop_statement",
    c"loop_statement",
    c"while_loop_statement",
    c"case_statement",
    c"select_statement",
]);
static ADA_VAR_TYPES: Kinds<2> =
    kinds([c"object_declaration", c"number_declaration"]);
static ADA_ASSIGN_TYPES: Kinds<1> = kinds([c"assignment_statement"]);
static ADA_THROW_TYPES: Kinds<1> = kinds([c"raise_statement"]);
static ADA_MODULE_TYPES: Kinds<1> = kinds([c"compilation"]);
static AGDA_FUNC_TYPES: Kinds<1> = kinds([c"function"]);
static AGDA_CLASS_TYPES: Kinds<2> = kinds([c"data", c"record"]);
static AGDA_CALL_TYPES: Kinds<2> = kinds([c"module_application", c"expr"]);
static AGDA_IMPORT_TYPES: Kinds<4> =
    kinds([c"import", c"open", c"import_directive", c"instance"]);
static AGDA_BRANCH_TYPES: Kinds<3> = kinds([c"lambda", c"match", c"do"]);
static AGDA_VAR_TYPES: Kinds<1> = kinds([c"typed_binding"]);
static AGDA_MODULE_TYPES: Kinds<1> = kinds([c"source_file"]);
static RACKET_CLASS_TYPES: Kinds<1> = kinds([c"structure"]);

// Racket def-forms (`(define (f ..) ..)`) are `list` nodes; the def-vs-call
// gate is in cbm_resolve_func_name (returns NULL for a non-def list).
static RACKET_FUNC_TYPES: Kinds<1> = kinds([c"list"]);
static RACKET_CALL_TYPES: Kinds<1> = kinds([c"list"]);
static RACKET_VAR_TYPES: Kinds<1> = kinds([c"symbol"]);
static RACKET_MODULE_TYPES: Kinds<1> = kinds([c"program"]);
static ODIN_FUNC_TYPES: Kinds<2> = kinds([
    c"procedure_declaration",
    c"overloaded_procedure_declaration",
]);
static ODIN_CLASS_TYPES: Kinds<4> = kinds([
    c"struct_declaration",
    c"enum_declaration",
    c"union_declaration",
    c"package_declaration",
]);
static ODIN_FIELD_TYPES: Kinds<1> = kinds([c"field_declaration"]);
static ODIN_CALL_TYPES: Kinds<2> =
    kinds([c"call_expression", c"selector_call_expression"]);
static ODIN_IMPORT_TYPES: Kinds<3> =
    kinds([c"import_declaration", c"import", c"using_statement"]);
static ODIN_BRANCH_TYPES: Kinds<4> = kinds([
    c"if_statement",
    c"for_statement",
    c"switch_statement",
    c"when_statement",
]);
static ODIN_VAR_TYPES: Kinds<2> =
    kinds([c"variable_declaration", c"const_declaration"]);
static ODIN_ASSIGN_TYPES: Kinds<1> = kinds([c"assignment_statement"]);
static ODIN_MODULE_TYPES: Kinds<1> = kinds([c"source_file"]);
static RESCRIPT_FUNC_TYPES: Kinds<1> = kinds([c"function"]);
static RESCRIPT_CALL_TYPES: Kinds<1> = kinds([c"call_expression"]);
static RESCRIPT_IMPORT_TYPES: Kinds<3> =
    kinds([c"open_statement", c"include_statement", c"include"]);
static RESCRIPT_BRANCH_TYPES: Kinds<3> =
    kinds([c"if_expression", c"switch_expression", c"try_expression"]);
static RESCRIPT_VAR_TYPES: Kinds<1> = kinds([c"let_binding"]);
static RESCRIPT_ASSIGN_TYPES: Kinds<2> =
    kinds([c"let_declaration", c"mutation_expression"]);
static RESCRIPT_THROW_TYPES: Kinds<1> = kinds([c"exception_declaration"]);
static RESCRIPT_DECORATOR_TYPES: Kinds<1> = kinds([c"decorator"]);
static RESCRIPT_CLASS_TYPES: Kinds<2> =
    kinds([c"module_declaration", c"type_declaration"]);
static RESCRIPT_MODULE_TYPES: Kinds<1> = kinds([c"source_file"]);
static PURESCRIPT_FUNC_TYPES: Kinds<1> = kinds([c"function"]);
static PURESCRIPT_CLASS_TYPES: Kinds<4> =
    kinds([c"class_declaration", c"data", c"newtype", c"type_alias"]);
static PURESCRIPT_CALL_TYPES: Kinds<1> = kinds([c"exp_apply"]);
static PURESCRIPT_IMPORT_TYPES: Kinds<3> =
    kinds([c"import", c"import_item", c"instance"]);
static PURESCRIPT_BRANCH_TYPES: Kinds<3> =
    kinds([c"exp_if", c"exp_case", c"exp_do"]);
static PURESCRIPT_VAR_TYPES: Kinds<1> = kinds([c"signature"]);
static PURESCRIPT_MODULE_TYPES: Kinds<1> = kinds([c"module"]);

// The lambda node is `fun_expr` (the bare `fun` is only the keyword token, never
// a named node); its name lives on the enclosing let_binding's `pat` field, so
// cbm_resolve_func_name climbs to the parent for naming. A function application
// (`f x y`) is an `applicative` node — `infix_expr` is binary-operator
// application (`a + b`), not a call.
static NICKEL_FUNC_TYPES: Kinds<1> = kinds([c"fun_expr"]);
static NICKEL_CALL_TYPES: Kinds<1> = kinds([c"applicative"]);
static NICKEL_IMPORT_TYPES: Kinds<2> = kinds([c"import", c"include"]);
static NICKEL_BRANCH_TYPES: Kinds<2> = kinds([c"if", c"match"]);
static NICKEL_VAR_TYPES: Kinds<1> = kinds([c"let"]);
static NICKEL_MODULE_TYPES: Kinds<1> = kinds([c"source_file"]);
static CRYSTAL_FUNC_TYPES: Kinds<2> =
    kinds([c"method_def", c"abstract_method_def"]);
static CRYSTAL_CLASS_TYPES: Kinds<6> = kinds([
    c"class_def",
    c"struct_def",
    c"module_def",
    c"enum_def",
    c"annotation_def",
    c"type_declaration",
]);
static CRYSTAL_FIELD_TYPES: Kinds<2> = kinds([c"instance_var", c"class_var"]);
static CRYSTAL_CALL_TYPES: Kinds<3> =
    kinds([c"call", c"command", c"implicit_object_call"]);
static CRYSTAL_IMPORT_TYPES: Kinds<1> = kinds([c"require"]);
static CRYSTAL_BRANCH_TYPES: Kinds<7> = kinds([
    c"if",
    c"unless",
    c"case",
    c"while",
    c"until",
    c"begin_block",
    c"rescue_block",
]);
static CRYSTAL_VAR_TYPES: Kinds<2> = kinds([c"assign", c"multi_assign"]);
static CRYSTAL_ASSIGN_TYPES: Kinds<2> = kinds([c"assign", c"operator_assign"]);
static CRYSTAL_DECORATOR_TYPES: Kinds<1> = kinds([c"annotation"]);
static CRYSTAL_MODULE_TYPES: Kinds<1> = kinds([c"program"]);
static TEAL_FUNC_TYPES: Kinds<3> = kinds([
    c"function_statement",
    c"anon_function",
    c"function_signature",
]);
static TEAL_CLASS_TYPES: Kinds<4> = kinds([
    c"record_declaration",
    c"interface_declaration",
    c"enum_declaration",
    c"type_declaration",
]);
static TEAL_CALL_TYPES: Kinds<1> = kinds([c"function_call"]);
static TEAL_BRANCH_TYPES: Kinds<5> = kinds([
    c"if_statement",
    c"while_statement",
    c"repeat_statement",
    c"for_statement",
    c"generic_for_statement",
]);
static TEAL_VAR_TYPES: Kinds<1> = kinds([c"var_declaration"]);
static TEAL_ASSIGN_TYPES: Kinds<1> = kinds([c"var_assignment"]);
static TEAL_MODULE_TYPES: Kinds<1> = kinds([c"program"]);
static HARE_FUNC_TYPES: Kinds<1> = kinds([c"function_declaration"]);
static HARE_CLASS_TYPES: Kinds<1> = kinds([c"type_declaration"]);
static HARE_CALL_TYPES: Kinds<1> = kinds([c"call_expression"]);
static HARE_IMPORT_TYPES: Kinds<1> = kinds([c"use_statement"]);
static HARE_BRANCH_TYPES: Kinds<4> = kinds([
    c"if_statement",
    c"for_statement",
    c"switch_expression",
    c"match_expression",
]);
static HARE_VAR_TYPES: Kinds<2> =
    kinds([c"let_declaration", c"const_declaration"]);
static HARE_ASSIGN_TYPES: Kinds<1> = kinds([c"assignment_expression"]);
static HARE_MODULE_TYPES: Kinds<1> = kinds([c"source_file"]);
static PONY_FUNC_TYPES: Kinds<4> = kinds([
    c"method",
    c"constructor",
    c"ffi_method",
    c"lambda_expression",
]);
static PONY_CLASS_TYPES: Kinds<7> = kinds([
    c"actor_definition",
    c"class_definition",
    c"struct_definition",
    c"trait_definition",
    c"interface_definition",
    c"primitive_definition",
    c"type_alias",
]);
static PONY_CALL_TYPES: Kinds<1> = kinds([c"call_expression"]);
static PONY_IMPORT_TYPES: Kinds<1> = kinds([c"use_statement"]);
static PONY_BRANCH_TYPES: Kinds<6> = kinds([
    c"if_statement",
    c"match_statement",
    c"for_statement",
    c"while_statement",
    c"repeat_statement",
    c"try_statement",
]);
static PONY_VAR_TYPES: Kinds<1> = kinds([c"variable_declaration"]);
static PONY_ASSIGN_TYPES: Kinds<1> = kinds([c"assignment_expression"]);
static PONY_MODULE_TYPES: Kinds<1> = kinds([c"source_file"]);
static LUAU_FUNC_TYPES: Kinds<2> =
    kinds([c"function_declaration", c"function_definition"]);
static LUAU_CLASS_TYPES: Kinds<1> = kinds([c"type_definition"]);
static LUAU_CALL_TYPES: Kinds<1> = kinds([c"function_call"]);
static LUAU_BRANCH_TYPES: Kinds<4> = kinds([
    c"if_statement",
    c"while_statement",
    c"repeat_statement",
    c"for_statement",
]);
static LUAU_VAR_TYPES: Kinds<1> = kinds([c"variable_declaration"]);
static LUAU_ASSIGN_TYPES: Kinds<1> = kinds([c"assignment_statement"]);
static LUAU_MODULE_TYPES: Kinds<1> = kinds([c"program"]);
static JANET_MODULE_TYPES: Kinds<1> = kinds([c"source"]);
static SWAY_FUNC_TYPES: Kinds<3> = kinds([
    c"function_item",
    c"closure_expression",
    c"function_signature_item",
]);
static SWAY_CLASS_TYPES: Kinds<5> = kinds([
    c"struct_item",
    c"enum_item",
    c"trait_item",
    c"impl_item",
    c"abi_item",
]);
static SWAY_CALL_TYPES: Kinds<2> =
    kinds([c"call_expression", c"abi_call_expression"]);
static SWAY_IMPORT_TYPES: Kinds<1> = kinds([c"use_declaration"]);
static SWAY_BRANCH_TYPES: Kinds<4> = kinds([
    c"if_expression",
    c"match_expression",
    c"while_expression",
    c"for_expression",
]);
static SWAY_VAR_TYPES: Kinds<2> = kinds([c"let_declaration", c"const_item"]);
static SWAY_ASSIGN_TYPES: Kinds<1> = kinds([c"assignment_expression"]);
static SWAY_MODULE_TYPES: Kinds<1> = kinds([c"source_file"]);
static NASM_FUNC_TYPES: Kinds<3> =
    kinds([c"label", c"preproc_def", c"preproc_multiline_macro"]);
static NASM_CLASS_TYPES: Kinds<1> = kinds([c"struc_declaration"]);
static NASM_CALL_TYPES: Kinds<2> =
    kinds([c"call_syntax_expression", c"actual_instruction"]);
static NASM_IMPORT_TYPES: Kinds<1> = kinds([c"preproc_include"]);
static NASM_VAR_TYPES: Kinds<1> = kinds([c"label"]);
static NASM_MODULE_TYPES: Kinds<1> = kinds([c"source_file"]);
static ASSEMBLY_FUNC_TYPES: Kinds<1> = kinds([c"label"]);
static ASSEMBLY_VAR_TYPES: Kinds<1> = kinds([c"label"]);
static ASSEMBLY_MODULE_TYPES: Kinds<1> = kinds([c"program"]);
static ASTRO_MODULE_TYPES: Kinds<1> = kinds([c"document"]);
static BLADE_MODULE_TYPES: Kinds<1> = kinds([c"document"]);
static JUST_FUNC_TYPES: Kinds<1> = kinds([c"recipe"]);
static JUST_CALL_TYPES: Kinds<2> = kinds([c"function_call", c"dependency"]);
static JUST_IMPORT_TYPES: Kinds<1> = kinds([c"import"]);
static JUST_BRANCH_TYPES: Kinds<1> = kinds([c"if_expression"]);
static JUST_ASSIGN_TYPES: Kinds<1> = kinds([c"assignment"]);
static JUST_MODULE_TYPES: Kinds<1> = kinds([c"source_file"]);
static GOTEMPLATE_FUNC_TYPES: Kinds<1> = kinds([c"define_action"]);
static GOTEMPLATE_CALL_TYPES: Kinds<3> =
    kinds([c"function_call", c"method_call", c"template_action"]);
static GOTEMPLATE_MODULE_TYPES: Kinds<1> = kinds([c"template"]);
static TEMPL_FUNC_TYPES: Kinds<3> = kinds([
    c"function_declaration",
    c"method_declaration",
    c"method_elem",
]);
static TEMPL_CLASS_TYPES: Kinds<4> = kinds([
    c"component_declaration",
    c"type_alias",
    c"type_declaration",
    c"type_spec",
]);
static TEMPL_CALL_TYPES: Kinds<1> = kinds([c"call_expression"]);
static TEMPL_IMPORT_TYPES: Kinds<2> = kinds([c"import", c"import_declaration"]);
static TEMPL_MODULE_TYPES: Kinds<1> = kinds([c"source_file"]);
static LIQUID_IMPORT_TYPES: Kinds<2> = kinds([c"include", c"include_statement"]);
static LIQUID_MODULE_TYPES: Kinds<1> = kinds([c"template"]);
static JINJA2_MODULE_TYPES: Kinds<1> = kinds([c"source_file"]);
static PRISMA_CLASS_TYPES: Kinds<6> = kinds([
    c"model_declaration",
    c"enum_declaration",
    c"type_declaration",
    c"generator_declaration",
    c"datasource_declaration",
    c"view_declaration",
]);
static PRISMA_FIELD_TYPES: Kinds<1> = kinds([c"column_declaration"]);
static PRISMA_CALL_TYPES: Kinds<1> = kinds([c"call_expression"]);
static PRISMA_MODULE_TYPES: Kinds<1> = kinds([c"program"]);
static HYPRLANG_MODULE_TYPES: Kinds<1> = kinds([c"source_file"]);
static DOTENV_MODULE_TYPES: Kinds<1> = kinds([c"source_file"]);
static DIFF_CALL_TYPES: Kinds<0> = kinds([]);
static DIFF_MODULE_TYPES: Kinds<1> = kinds([c"source"]);
static WGSL_FUNC_TYPES: Kinds<1> = kinds([c"function_declaration"]);
static WGSL_CLASS_TYPES: Kinds<3> = kinds([
    c"struct_declaration",
    c"type_alias_declaration",
    c"type_declaration",
]);
static WGSL_CALL_TYPES: Kinds<1> =
    kinds([c"type_constructor_or_function_call_expression"]);
static WGSL_IMPORT_TYPES: Kinds<1> = kinds([c"enable_directive"]);
static WGSL_BRANCH_TYPES: Kinds<5> = kinds([
    c"if_statement",
    c"switch_statement",
    c"for_statement",
    c"while_statement",
    c"loop_statement",
]);
static WGSL_VAR_TYPES: Kinds<1> = kinds([c"variable_declaration"]);
static WGSL_ASSIGN_TYPES: Kinds<1> = kinds([c"assignment_statement"]);
static WGSL_MODULE_TYPES: Kinds<1> = kinds([c"translation_unit"]);
static KDL_MODULE_TYPES: Kinds<1> = kinds([c"document"]);
static JSON5_MODULE_TYPES: Kinds<1> = kinds([c"document"]);
static JSONNET_FUNC_TYPES: Kinds<2> = kinds([c"anonymous_function", c"bind"]);
static JSONNET_CALL_TYPES: Kinds<1> = kinds([c"functioncall"]);
static JSONNET_IMPORT_TYPES: Kinds<2> = kinds([c"import", c"importstr"]);
static JSONNET_BRANCH_TYPES: Kinds<1> = kinds([c"conditional"]);
static JSONNET_VAR_TYPES: Kinds<1> = kinds([c"local_bind"]);
static JSONNET_MODULE_TYPES: Kinds<1> = kinds([c"document"]);
static RON_MODULE_TYPES: Kinds<1> = kinds([c"source_file"]);
static THRIFT_FUNC_TYPES: Kinds<2> =
    kinds([c"function_definition", c"service_definition"]);
static THRIFT_CLASS_TYPES: Kinds<7> = kinds([
    c"struct_definition",
    c"union_definition",
    c"enum_definition",
    c"exception_definition",
    c"interaction_definition",
    c"package_declaration",
    c"service_definition",
]);
static THRIFT_FIELD_TYPES: Kinds<1> = kinds([c"field"]);
static THRIFT_IMPORT_TYPES: Kinds<4> = kinds([
    c"include_statement",
    c"extends",
    c"include",
    c"namespace_declaration",
]);
static THRIFT_VAR_TYPES: Kinds<1> = kinds([c"const_definition"]);
static THRIFT_MODULE_TYPES: Kinds<1> = kinds([c"document"]);
static CAPNP_FUNC_TYPES: Kinds<1> = kinds([c"method"]);

// custom_type (a type REFERENCE inside field_type) and type_definition (LHS of a
// `using X = ...` directive) are not top-level type defs — including them would
// mint spurious Class nodes for every typed field/return.
static CAPNP_CLASS_TYPES: Kinds<3> = kinds([c"struct", c"enum", c"interface"]);
static CAPNP_FIELD_TYPES: Kinds<1> = kinds([c"field"]);
static CAPNP_IMPORT_TYPES: Kinds<3> =
    kinds([c"import", c"extends", c"using_directive"]);
static CAPNP_VAR_TYPES: Kinds<1> = kinds([c"const"]);
static CAPNP_MODULE_TYPES: Kinds<1> = kinds([c"source"]);
static PROPERTIES_VAR_TYPES: Kinds<1> = kinds([c"property"]);

// tree-sitter-properties roots the tree at `file`, not `source_file`.
static PROPERTIES_MODULE_TYPES: Kinds<2> = kinds([c"file", c"source_file"]);
static SSHCONFIG_MODULE_TYPES: Kinds<1> = kinds([c"source_file"]);
static BIBTEX_CALL_TYPES: Kinds<0> = kinds([]);
static BIBTEX_MODULE_TYPES: Kinds<1> = kinds([c"document"]);
static STARLARK_FUNC_TYPES: Kinds<2> =
    kinds([c"function_definition", c"lambda"]);
static STARLARK_CALL_TYPES: Kinds<1> = kinds([c"call"]);
static STARLARK_BRANCH_TYPES: Kinds<2> =
    kinds([c"if_statement", c"for_statement"]);
static STARLARK_VAR_TYPES: Kinds<1> = kinds([c"identifier"]);
static STARLARK_ASSIGN_TYPES: Kinds<2> =
    kinds([c"assignment", c"augmented_assignment"]);
static STARLARK_IMPORT_TYPES: Kinds<1> = kinds([c"with_clause"]);
static STARLARK_MODULE_TYPES: Kinds<1> = kinds([c"module"]);
static BICEP_FUNC_TYPES: Kinds<2> =
    kinds([c"user_defined_function", c"lambda_expression"]);
static BICEP_CLASS_TYPES: Kinds<3> = kinds([
    c"resource_declaration",
    c"type_declaration",
    c"module_declaration",
]);
static BICEP_IMPORT_TYPES: Kinds<4> = kinds([
    c"import_statement",
    c"module_declaration",
    c"import",
    c"using_statement",
]);
static BICEP_VAR_TYPES: Kinds<3> = kinds([
    c"variable_declaration",
    c"parameter_declaration",
    c"output_declaration",
]);
static BICEP_CALL_TYPES: Kinds<1> = kinds([c"call_expression"]);
static BICEP_MODULE_TYPES: Kinds<1> = kinds([c"program"]);
static CSV_MODULE_TYPES: Kinds<1> = kinds([c"document"]);
static REQUIREMENTS_MODULE_TYPES: Kinds<1> = kinds([c"file"]);
static HLSL_FUNC_TYPES: Kinds<2> =
    kinds([c"function_definition", c"lambda_expression"]);
static HLSL_CLASS_TYPES: Kinds<6> = kinds([
    c"class_specifier",
    c"struct_specifier",
    c"enum_specifier",
    c"module_declaration",
    c"type_definition",
    c"union_specifier",
]);
static HLSL_CALL_TYPES: Kinds<2> =
    kinds([c"call_expression", c"new_expression"]);
static HLSL_IMPORT_TYPES: Kinds<3> =
    kinds([c"preproc_include", c"import", c"import_declaration"]);
static HLSL_BRANCH_TYPES: Kinds<5> = kinds([
    c"if_statement",
    c"for_statement",
    c"while_statement",
    c"do_statement",
    c"switch_statement",
]);
static HLSL_VAR_TYPES: Kinds<1> = kinds([c"declaration"]);
static HLSL_ASSIGN_TYPES: Kinds<1> = kinds([c"assignment_expression"]);
static HLSL_MODULE_TYPES: Kinds<1> = kinds([c"translation_unit"]);
static VHDL_CLASS_TYPES: Kinds<8> = kinds([
    c"entity_declaration",
    c"architecture_definition",
    c"component_declaration",
    c"interface_declaration",
    c"package_declaration",
    c"protected_type_declaration",
    c"record_type_definition",
    c"type_declaration",
]);
static VHDL_CALL_TYPES: Kinds<4> = kinds([
    c"function_call",
    c"procedure_call_statement",
    c"component_instantiation_statement",
    c"parenthesis_group",
]);
static VHDL_IMPORT_TYPES: Kinds<2> = kinds([c"library_clause", c"use_clause"]);
static VHDL_BRANCH_TYPES: Kinds<3> =
    kinds([c"if_statement", c"case_statement", c"loop_statement"]);
static VHDL_VAR_TYPES: Kinds<3> = kinds([
    c"variable_declaration",
    c"signal_declaration",
    c"constant_declaration",
]);
static VHDL_ASSIGN_TYPES: Kinds<2> =
    kinds([c"variable_assignment", c"signal_assignment"]);
static VHDL_FUNC_TYPES: Kinds<2> =
    kinds([c"subprogram_declaration", c"subprogram_definition"]);
static VHDL_MODULE_TYPES: Kinds<1> = kinds([c"design_file"]);
static SYSTEMVERILOG_FUNC_TYPES: Kinds<4> = kinds([
    c"function_declaration",
    c"task_declaration",
    c"function_body_declaration",
    c"function_statement",
]);
static SYSTEMVERILOG_CLASS_TYPES: Kinds<6> = kinds([
    c"class_declaration",
    c"module_declaration",
    c"interface_declaration",
    c"library_declaration",
    c"package_declaration",
    c"type_declaration",
]);
static SYSTEMVERILOG_CALL_TYPES: Kinds<3> = kinds([
    c"function_subroutine_call",
    c"system_tf_call",
    c"method_call",
]);
static SYSTEMVERILOG_IMPORT_TYPES: Kinds<7> = kinds([
    c"package_import_declaration",
    c"extends",
    c"import",
    c"include",
    c"include_statement",
    c"instance",
    c"use_clause",
]);
static SYSTEMVERILOG_BRANCH_TYPES: Kinds<2> = kinds([c"case_statement", c"if"]);
static SYSTEMVERILOG_VAR_TYPES: Kinds<2> = kinds([c"parameter", c"localparam"]);
static SYSTEMVERILOG_MODULE_TYPES: Kinds<1> = kinds([c"source_file"]);
static DEVICETREE_CALL_TYPES: Kinds<1> = kinds([c"call_expression"]);
static DEVICETREE_IMPORT_TYPES: Kinds<2> =
    kinds([c"dtsi_include", c"preproc_include"]);
static DEVICETREE_MODULE_TYPES: Kinds<1> = kinds([c"document"]);
static LINKERSCRIPT_CALL_TYPES: Kinds<1> = kinds([c"call_expression"]);
static LINKERSCRIPT_MODULE_TYPES: Kinds<1> = kinds([c"linkerscript"]);
static GN_CALL_TYPES: Kinds<1> = kinds([c"call_expression"]);
static GN_IMPORT_TYPES: Kinds<2> = kinds([c"import_statement", c"import"]);
static GN_BRANCH_TYPES: Kinds<2> =
    kinds([c"if_statement", c"foreach_statement"]);
static GN_ASSIGN_TYPES: Kinds<1> = kinds([c"assignment_statement"]);
static GN_MODULE_TYPES: Kinds<1> = kinds([c"source_file"]);
static KCONFIG_CLASS_TYPES: Kinds<4> =
    kinds([c"config", c"menuconfig", c"choice", c"type_definition"]);
static KCONFIG_IMPORT_TYPES: Kinds<1> = kinds([c"source"]);
static KCONFIG_BRANCH_TYPES: Kinds<1> = kinds([c"if"]);
static KCONFIG_MODULE_TYPES: Kinds<1> = kinds([c"source"]);

// `anonymous_python_function` is the tree-sitter-bitbake node for a
// `python do_foo() {...}` task; `function_definition` is a `do_foo() {...}`
// shell task. (`recipe` is the file root, not a function.)
static BITBAKE_FUNC_TYPES: Kinds<2> =
    kinds([c"function_definition", c"anonymous_python_function"]);
static BITBAKE_VAR_TYPES: Kinds<1> = kinds([c"variable_assignment"]);
static BITBAKE_CALL_TYPES: Kinds<1> = kinds([c"call"]);
static BITBAKE_IMPORT_TYPES: Kinds<8> = kinds([
    c"export_statement",
    c"import",
    c"import_from_statement",
    c"import_statement",
    c"include",
    c"include_directive",
    c"require",
    c"with_clause",
]);
static BITBAKE_MODULE_TYPES: Kinds<1> = kinds([c"source_file"]);
static SMALI_FUNC_TYPES: Kinds<1> = kinds([c"method_definition"]);
static SMALI_CLASS_TYPES: Kinds<1> = kinds([c"class_definition"]);
static SMALI_FIELD_TYPES: Kinds<1> = kinds([c"field_definition"]);
static SMALI_IMPORT_TYPES: Kinds<2> =
    kinds([c"super_directive", c"implements_directive"]);
static SMALI_MODULE_TYPES: Kinds<1> = kinds([c"source_file"]);
static TABLEGEN_FUNC_TYPES: Kinds<3> = kinds([c"def", c"multiclass", c"defm"]);
static TABLEGEN_CLASS_TYPES: Kinds<1> = kinds([c"class"]);
static TABLEGEN_IMPORT_TYPES: Kinds<2> =
    kinds([c"include", c"include_directive"]);
static TABLEGEN_MODULE_TYPES: Kinds<1> = kinds([c"source_file"]);
static ISPC_FUNC_TYPES: Kinds<1> = kinds([c"function_definition"]);
static ISPC_CLASS_TYPES: Kinds<4> = kinds([
    c"struct_specifier",
    c"enum_specifier",
    c"type_definition",
    c"union_specifier",
]);
static ISPC_CALL_TYPES: Kinds<2> =
    kinds([c"call_expression", c"new_expression"]);
static ISPC_BRANCH_TYPES: Kinds<3> =
    kinds([c"if_statement", c"switch_statement", c"while_statement"]);
static ISPC_VAR_TYPES: Kinds<1> = kinds([c"declaration"]);
static ISPC_ASSIGN_TYPES: Kinds<1> = kinds([c"assignment_expression"]);
static ISPC_IMPORT_TYPES: Kinds<1> = kinds([c"preproc_include"]);
static ISPC_MODULE_TYPES: Kinds<1> = kinds([c"translation_unit"]);
static CAIRO_FUNC_TYPES: Kinds<2> =
    kinds([c"function_definition", c"function_signature"]);
static CAIRO_CLASS_TYPES: Kinds<6> = kinds([
    c"struct_definition",
    c"enum_item",
    c"trait_item",
    c"impl_item",
    c"struct_item",
    c"type_definition",
]);
static CAIRO_CALL_TYPES: Kinds<2> = kinds([c"call_expression", c"call"]);
static CAIRO_IMPORT_TYPES: Kinds<3> =
    kinds([c"use_declaration", c"import", c"import_statement"]);
static CAIRO_BRANCH_TYPES: Kinds<3> =
    kinds([c"if_expression", c"match_expression", c"loop_expression"]);
static CAIRO_VAR_TYPES: Kinds<2> = kinds([c"let_declaration", c"const_item"]);
static CAIRO_ASSIGN_TYPES: Kinds<1> = kinds([c"assignment_expression"]);
static CAIRO_MODULE_TYPES: Kinds<1> = kinds([c"source_file"]);
static MOVE_FUNC_TYPES: Kinds<1> = kinds([c"function_item"]);

// This vendored move grammar models only function_item + module as named defs;
// "struct"/"enum" exist only as anonymous keyword tokens, never as parent nodes,
// so there is no class/struct/enum definition node to match.
static MOVE_CALL_TYPES: Kinds<1> = kinds([c"call_expression"]);
static MOVE_IMPORT_TYPES: Kinds<1> = kinds([c"use_declaration"]);
static MOVE_BRANCH_TYPES: Kinds<3> =
    kinds([c"if_expression", c"while_expression", c"loop_expression"]);
static MOVE_VAR_TYPES: Kinds<2> = kinds([c"let", c"const"]);
static MOVE_ASSIGN_TYPES: Kinds<1> = kinds([c"assignment_expression"]);
static MOVE_MODULE_TYPES: Kinds<1> = kinds([c"source_file"]);
static SQUIRREL_FUNC_TYPES: Kinds<3> = kinds([
    c"function_declaration",
    c"anonymous_function",
    c"lambda_expression",
]);
static SQUIRREL_CLASS_TYPES: Kinds<2> =
    kinds([c"class_declaration", c"enum_declaration"]);
static SQUIRREL_CALL_TYPES: Kinds<1> = kinds([c"call_expression"]);
static SQUIRREL_BRANCH_TYPES: Kinds<3> =
    kinds([c"if_statement", c"switch_statement", c"while_statement"]);
static SQUIRREL_VAR_TYPES: Kinds<1> = kinds([c"local_declaration"]);
static SQUIRREL_ASSIGN_TYPES: Kinds<1> = kinds([c"assignment_expression"]);
static SQUIRREL_IMPORT_TYPES: Kinds<1> = kinds([c"extends"]);
static SQUIRREL_MODULE_TYPES: Kinds<1> = kinds([c"source_file"]);
static FUNC_FUNC_TYPES: Kinds<1> = kinds([c"function_definition"]);
static FUNC_CALL_TYPES: Kinds<2> =
    kinds([c"method_call", c"function_application"]);
static FUNC_IMPORT_TYPES: Kinds<1> = kinds([c"include_directive"]);
static FUNC_MODULE_TYPES: Kinds<1> = kinds([c"source_file"]);
static REGEX_MODULE_TYPES: Kinds<1> = kinds([c"pattern"]);
static JSDOC_MODULE_TYPES: Kinds<1> = kinds([c"document"]);
static RST_MODULE_TYPES: Kinds<1> = kinds([c"document"]);
static BEANCOUNT_IMPORT_TYPES: Kinds<1> = kinds([c"include"]);
static BEANCOUNT_MODULE_TYPES: Kinds<1> = kinds([c"file"]);
static MERMAID_MODULE_TYPES: Kinds<1> = kinds([c"source_file"]);
static PUPPET_FUNC_TYPES: Kinds<2> = kinds([c"function_declaration", c"lambda"]);
static PUPPET_CLASS_TYPES: Kinds<4> = kinds([
    c"class_definition",
    c"node_definition",
    c"resource_declaration",
    c"type_declaration",
]);
static PUPPET_CALL_TYPES: Kinds<2> =
    kinds([c"function_call", c"include_statement"]);
static PUPPET_IMPORT_TYPES: Kinds<4> = kinds([
    c"include_statement",
    c"require_statement",
    c"include",
    c"require",
]);
static PUPPET_BRANCH_TYPES: Kinds<3> =
    kinds([c"if_statement", c"unless_statement", c"case_statement"]);
static PUPPET_VAR_TYPES: Kinds<1> = kinds([c"variable"]);
static PUPPET_ASSIGN_TYPES: Kinds<1> = kinds([c"assignment"]);
static PUPPET_MODULE_TYPES: Kinds<1> = kinds([c"source_file"]);
static PO_MODULE_TYPES: Kinds<1> = kinds([c"source_file"]);
static GITATTRIBUTES_MODULE_TYPES: Kinds<1> = kinds([c"source"]);
static GITIGNORE_MODULE_TYPES: Kinds<1> = kinds([c"document"]);
static SLANG_FUNC_TYPES: Kinds<2> =
    kinds([c"function_definition", c"lambda_expression"]);
static SLANG_CLASS_TYPES: Kinds<6> = kinds([
    c"class_specifier",
    c"enum_specifier",
    c"module_declaration",
    c"struct_specifier",
    c"type_definition",
    c"union_specifier",
]);
static SLANG_CALL_TYPES: Kinds<2> =
    kinds([c"call_expression", c"new_expression"]);
static SLANG_IMPORT_TYPES: Kinds<4> = kinds([
    c"import",
    c"import_declaration",
    c"import_statement",
    c"preproc_include",
]);
static SLANG_MODULE_TYPES: Kinds<1> = kinds([c"source_file"]);
static LLVM_MODULE_TYPES: Kinds<1> = kinds([c"source_file"]);
static SMITHY_FUNC_TYPES: Kinds<3> = kinds([
    c"operation_statement",
    c"service_statement",
    c"resource_statement",
]);
static SMITHY_CLASS_TYPES: Kinds<3> = kinds([
    c"structure_statement",
    c"union_statement",
    c"enum_statement",
]);
static SMITHY_FIELD_TYPES: Kinds<1> = kinds([c"shape_member"]);
static SMITHY_IMPORT_TYPES: Kinds<1> = kinds([c"use_statement"]);
static SMITHY_MODULE_TYPES: Kinds<1> = kinds([c"source_file"]);
static WIT_FUNC_TYPES: Kinds<4> = kinds([
    c"func_item",
    c"resource_method",
    c"export_item",
    c"import_item",
]);
static WIT_CLASS_TYPES: Kinds<5> = kinds([
    c"record_item",
    c"resource_item",
    c"enum_items",
    c"variant_items",
    c"flags_items",
]);
static WIT_FIELD_TYPES: Kinds<1> = kinds([c"record_field"]);
static WIT_IMPORT_TYPES: Kinds<6> = kinds([
    c"import_item",
    c"toplevel_use_item",
    c"export_item",
    c"import",
    c"include",
    c"include_item",
]);
static WIT_MODULE_TYPES: Kinds<1> = kinds([c"source_file"]);
static TLAPLUS_FUNC_TYPES: Kinds<2> =
    kinds([c"operator_definition", c"function_definition"]);
static TLAPLUS_CALL_TYPES: Kinds<3> =
    kinds([c"function_evaluation", c"call", c"bound_op"]);
static TLAPLUS_IMPORT_TYPES: Kinds<2> = kinds([c"extends", c"instance"]);
static TLAPLUS_BRANCH_TYPES: Kinds<2> = kinds([c"if_then_else", c"case"]);
static TLAPLUS_VAR_TYPES: Kinds<1> = kinds([c"variable_declaration"]);
static TLAPLUS_MODULE_TYPES: Kinds<1> = kinds([c"source_file"]);
static PKL_FUNC_TYPES: Kinds<2> = kinds([c"classMethod", c"objectMethod"]);
static PKL_CLASS_TYPES: Kinds<2> = kinds([c"clazz", c"typeAlias"]);
static PKL_IMPORT_TYPES: Kinds<6> = kinds([
    c"importClause",
    c"importGlobClause",
    c"importExpr",
    c"extendsOrAmendsClause",
    c"extends",
    c"import",
]);
static PKL_VAR_TYPES: Kinds<2> = kinds([c"classProperty", c"objectProperty"]);
static PKL_MODULE_TYPES: Kinds<1> = kinds([c"module"]);

// Both access exprs double as plain property reads; extract_pkl_callee keeps
// only the ones carrying an argumentList. `newExpr` resolves to its type.
static PKL_CALL_TYPES: Kinds<3> =
    kinds([c"unqualifiedAccessExpr", c"qualifiedAccessExpr", c"newExpr"]);

// Control-flow only, matching every other spec (short-circuit operators are
// deliberately excluded). `forGenerator` is also a loop — see helpers.c.
static PKL_BRANCH_TYPES: Kinds<3> =
    kinds([c"ifExpr", c"whenGenerator", c"forGenerator"]);
static PKL_THROW_TYPES: Kinds<1> = kinds([c"throwExpr"]);
static PKL_DECORATOR_TYPES: Kinds<1> = kinds([c"annotation"]);
static GOMOD_VAR_TYPES: Kinds<2> =
    kinds([c"require_directive", c"replace_directive"]);
static GOMOD_IMPORT_TYPES: Kinds<1> = kinds([c"require"]);
static GOMOD_MODULE_TYPES: Kinds<1> = kinds([c"source_file"]);
static APEX_FUNC_TYPES: Kinds<3> = kinds([
    c"method_declaration",
    c"constructor_declaration",
    c"trigger_declaration",
]);
static APEX_CLASS_TYPES: Kinds<3> = kinds([
    c"class_declaration",
    c"interface_declaration",
    c"enum_declaration",
]);
static APEX_FIELD_TYPES: Kinds<1> = kinds([c"field_declaration"]);
static APEX_CALL_TYPES: Kinds<1> = kinds([c"method_invocation"]);
static APEX_BRANCH_TYPES: Kinds<8> = kinds([
    c"if_statement",
    c"for_statement",
    c"while_statement",
    c"do_statement",
    c"switch_expression",
    c"try_statement",
    c"catch_clause",
    c"enhanced_for_statement",
]);
static APEX_VAR_TYPES: Kinds<1> = kinds([c"local_variable_declaration"]);
static APEX_ASSIGN_TYPES: Kinds<1> = kinds([c"assignment_expression"]);
static APEX_THROW_TYPES: Kinds<1> = kinds([c"throw_statement"]);
static APEX_DECORATOR_TYPES: Kinds<1> = kinds([c"annotation"]);
static APEX_IMPORT_TYPES: Kinds<2> = kinds([c"extends", c"with_clause"]);
static APEX_MODULE_TYPES: Kinds<1> = kinds([c"parser_output"]);
static SOQL_IMPORT_TYPES: Kinds<1> = kinds([c"with_clause"]);
static SOQL_MODULE_TYPES: Kinds<1> = kinds([c"source_file"]);
static SOSL_IMPORT_TYPES: Kinds<1> = kinds([c"with_clause"]);
static SOSL_MODULE_TYPES: Kinds<1> = kinds([c"source_file"]);
static MAKE_IMPORT_TYPES: Kinds<2> = kinds([c"include", c"include_directive"]);

// ==================== PINE SCRIPT ====================
// Node names verified against kvarenzn/tree-sitter-pine grammar.js.
static PINE_FUNC_TYPES: Kinds<1> = kinds([c"function_declaration_statement"]);
static PINE_CLASS_TYPES: Kinds<1> = kinds([c"type_definition_statement"]);
static PINE_MODULE_TYPES: Kinds<1> = kinds([c"source_file"]);
static PINE_CALL_TYPES: Kinds<1> = kinds([c"call"]);
static PINE_VAR_TYPES: Kinds<2> = kinds([
    c"variable_definition_statement",
    c"tuple_declaration_statement",
]);
static PINE_BRANCH_TYPES: Kinds<5> = kinds([
    c"if_statement",
    c"switch_statement",
    c"for_statement",
    c"for_in_statement",
    c"while_statement",
]);
static PINE_ASSIGN_TYPES: Kinds<1> = kinds([c"reassignment_statement"]);
static MOJO_FUNC_TYPES: Kinds<1> = kinds([c"function_definition"]);
static MOJO_CLASS_TYPES: Kinds<2> =
    kinds([c"class_definition", c"trait_definition"]);
static MOJO_MODULE_TYPES: Kinds<1> = kinds([c"module"]);
static MOJO_CALL_TYPES: Kinds<1> = kinds([c"call"]);
static MOJO_IMPORT_TYPES: Kinds<3> = kinds([
    c"import_statement",
    c"import_from_statement",
    c"future_import_statement",
]);
static MOJO_BRANCH_TYPES: Kinds<6> = kinds([
    c"if_statement",
    c"match_statement",
    c"for_statement",
    c"while_statement",
    c"try_statement",
    c"with_statement",
]);
static MOJO_VAR_TYPES: Kinds<1> = kinds([c"assignment"]);
static MOJO_ASSIGN_TYPES: Kinds<2> =
    kinds([c"assignment", c"augmented_assignment"]);

// ==================== PL/SQL ====================
// Node names verified against AndreasMaierDe/tree-sitter-plsql grammar.js.
static PLSQL_FUNC_TYPES: Kinds<6> = kinds([
    c"create_function",
    c"create_procedure",
    c"function_definition",
    c"procedure_definition",
    c"function_declaration",
    c"procedure_declaration",
]);
static PLSQL_CLASS_TYPES: Kinds<5> = kinds([
    c"create_package",
    c"create_package_body",
    c"create_type",
    c"create_type_body",
    c"create_trigger",
]);
static PLSQL_MODULE_TYPES: Kinds<1> = kinds([c"source_file"]);
static PLSQL_CALL_TYPES: Kinds<1> = kinds([c"ref_call"]);
static PLSQL_BRANCH_TYPES: Kinds<6> = kinds([
    c"if_statement",
    c"case_statement",
    c"basic_loop_statement",
    c"for_loop_statement",
    c"while_loop_statement",
    c"exception_handler",
]);
static PLSQL_ASSIGN_TYPES: Kinds<1> = kinds([c"assignment_statement"]);
static PLSQL_THROW_TYPES: Kinds<1> = kinds([c"raise_statement"]);

// InterSystems ObjectScript. Node names verified against
// intersystems/tree-sitter-objectscript grammar.
static OBJECTSCRIPT_UDL_FUNC_TYPES: Kinds<3> =
    kinds([c"method", c"classmethod", c"query"]);
static OBJECTSCRIPT_UDL_CLASS_TYPES: Kinds<1> = kinds([c"class_definition"]);
static OBJECTSCRIPT_UDL_FIELD_TYPES: Kinds<7> = kinds([
    c"property",
    c"parameter",
    c"index",
    c"trigger",
    c"xdata",
    c"storage",
    c"foreignkey",
]);
static OBJECTSCRIPT_UDL_CALL_TYPES: Kinds<4> = kinds([
    c"class_method_call",
    c"method_call",
    c"relative_dot_method",
    c"macro",
]);
static OBJECTSCRIPT_UDL_MODULE_TYPES: Kinds<1> = kinds([c"source_file"]);

// Branching nodes for cyclomatic complexity (verified against grammar node-types)
static OBJECTSCRIPT_UDL_BRANCH_TYPES: Kinds<5> = kinds([
    c"command_if",
    c"command_for",
    c"command_while",
    c"elseif_block",
    c"catch_block",
]);

// Parameterized entries are `procedure` nodes whose direct `tag` child names
// the callable; bare labels remain standalone `tag` nodes.
static OBJECTSCRIPT_ROUTINE_FUNC_TYPES: Kinds<2> = kinds([c"procedure", c"tag"]);
static OBJECTSCRIPT_ROUTINE_CALL_TYPES: Kinds<2> =
    kinds([c"extrinsic_function", c"routine_tag_call"]);
static OBJECTSCRIPT_ROUTINE_MODULE_TYPES: Kinds<1> = kinds([c"source_file"]);
// ==================== SPEC TABLE ====================

// The assignment kinds of the JavaScript family, written inline in the
// C table's rows.
static JS_ASSIGN_TYPES: Kinds<2> =
    kinds([c"assignment_expression", c"augmented_assignment_expression"]);

/// A language without a row: `cbm_lang_spec` returns NULL for it.
const NO_SPEC: CbmLangSpec = CbmLangSpec {
    language: 0,
    function_node_types: ptr::null(),
    class_node_types: ptr::null(),
    field_node_types: ptr::null(),
    module_node_types: ptr::null(),
    call_node_types: ptr::null(),
    import_node_types: ptr::null(),
    import_from_types: ptr::null(),
    branching_node_types: ptr::null(),
    variable_node_types: ptr::null(),
    assignment_node_types: ptr::null(),
    throw_node_types: ptr::null(),
    throws_clause_field: ptr::null(),
    decorator_node_types: ptr::null(),
    env_access_functions: ptr::null(),
    env_access_member_patterns: ptr::null(),
    ts_factory: None,
    embedded_imports: ptr::null(),
};

/// Defaults for a row: every kind list empty, every optional field NULL.
const LIST_DEFAULTS: CbmLangSpec = CbmLangSpec {
    language: 0,
    function_node_types: EMPTY_TYPES.as_ptr(),
    class_node_types: EMPTY_TYPES.as_ptr(),
    field_node_types: EMPTY_TYPES.as_ptr(),
    module_node_types: EMPTY_TYPES.as_ptr(),
    call_node_types: EMPTY_TYPES.as_ptr(),
    import_node_types: EMPTY_TYPES.as_ptr(),
    import_from_types: EMPTY_TYPES.as_ptr(),
    branching_node_types: EMPTY_TYPES.as_ptr(),
    variable_node_types: EMPTY_TYPES.as_ptr(),
    assignment_node_types: EMPTY_TYPES.as_ptr(),
    throw_node_types: EMPTY_TYPES.as_ptr(),
    throws_clause_field: ptr::null(),
    decorator_node_types: EMPTY_TYPES.as_ptr(),
    env_access_functions: ptr::null(),
    env_access_member_patterns: ptr::null(),
    ts_factory: None,
    embedded_imports: ptr::null(),
};

static LANG_SPECS: [CbmLangSpec; LANG_COUNT] = {
    let mut specs = [NO_SPEC; LANG_COUNT];
    specs[lang::GO as usize] = CbmLangSpec {
        language: lang::GO,
        function_node_types: GO_FUNC_TYPES.as_ptr(),
        class_node_types: GO_CLASS_TYPES.as_ptr(),
        field_node_types: GO_FIELD_TYPES.as_ptr(),
        module_node_types: GO_MODULE_TYPES.as_ptr(),
        call_node_types: GO_CALL_TYPES.as_ptr(),
        import_node_types: GO_IMPORT_TYPES.as_ptr(),
        import_from_types: GO_IMPORT_TYPES.as_ptr(),
        branching_node_types: GO_BRANCH_TYPES.as_ptr(),
        variable_node_types: GO_VAR_TYPES.as_ptr(),
        assignment_node_types: GO_ASSIGN_TYPES.as_ptr(),
        env_access_functions: GO_ENV_FUNCS.as_ptr(),
        ts_factory: Some(tree_sitter_go),
        ..LIST_DEFAULTS
    };
    specs[lang::PYTHON as usize] = CbmLangSpec {
        language: lang::PYTHON,
        function_node_types: PY_FUNC_TYPES.as_ptr(),
        class_node_types: PY_CLASS_TYPES.as_ptr(),
        module_node_types: PY_MODULE_TYPES.as_ptr(),
        call_node_types: PY_CALL_TYPES.as_ptr(),
        import_node_types: PY_IMPORT_TYPES.as_ptr(),
        import_from_types: PY_IMPORT_FROM_TYPES.as_ptr(),
        branching_node_types: PY_BRANCH_TYPES.as_ptr(),
        variable_node_types: PY_VAR_TYPES.as_ptr(),
        assignment_node_types: PY_VAR_TYPES.as_ptr(),
        throw_node_types: PY_THROW_TYPES.as_ptr(),
        decorator_node_types: PY_DECORATOR_TYPES.as_ptr(),
        env_access_functions: PY_ENV_FUNCS.as_ptr(),
        env_access_member_patterns: PY_ENV_MEMBERS.as_ptr(),
        ts_factory: Some(tree_sitter_python),
        ..LIST_DEFAULTS
    };
    specs[lang::JAVASCRIPT as usize] = CbmLangSpec {
        language: lang::JAVASCRIPT,
        function_node_types: JS_FUNC_TYPES.as_ptr(),
        class_node_types: JS_CLASS_TYPES.as_ptr(),
        module_node_types: JS_MODULE_TYPES.as_ptr(),
        call_node_types: JS_CALL_TYPES.as_ptr(),
        import_node_types: JS_IMPORT_TYPES.as_ptr(),
        import_from_types: JS_IMPORT_TYPES.as_ptr(),
        branching_node_types: JS_BRANCH_TYPES.as_ptr(),
        variable_node_types: JS_VAR_TYPES.as_ptr(),
        assignment_node_types: JS_ASSIGN_TYPES.as_ptr(),
        throw_node_types: JS_THROW_TYPES.as_ptr(),
        env_access_member_patterns: JS_ENV_MEMBERS.as_ptr(),
        ts_factory: Some(tree_sitter_javascript),
        ..LIST_DEFAULTS
    };
    specs[lang::TYPESCRIPT as usize] = CbmLangSpec {
        language: lang::TYPESCRIPT,
        function_node_types: TS_FUNC_TYPES.as_ptr(),
        class_node_types: TS_CLASS_TYPES.as_ptr(),
        module_node_types: JS_MODULE_TYPES.as_ptr(),
        call_node_types: JS_CALL_TYPES.as_ptr(),
        import_node_types: JS_IMPORT_TYPES.as_ptr(),
        import_from_types: JS_IMPORT_TYPES.as_ptr(),
        branching_node_types: JS_BRANCH_TYPES.as_ptr(),
        variable_node_types: JS_VAR_TYPES.as_ptr(),
        assignment_node_types: JS_ASSIGN_TYPES.as_ptr(),
        throw_node_types: JS_THROW_TYPES.as_ptr(),
        decorator_node_types: TS_DECORATOR_TYPES.as_ptr(),
        env_access_member_patterns: TS_ENV_MEMBERS.as_ptr(),
        ts_factory: Some(tree_sitter_typescript),
        ..LIST_DEFAULTS
    };
    specs[lang::TSX as usize] = CbmLangSpec {
        language: lang::TSX,
        function_node_types: TS_FUNC_TYPES.as_ptr(),
        class_node_types: TS_CLASS_TYPES.as_ptr(),
        module_node_types: JS_MODULE_TYPES.as_ptr(),
        call_node_types: JS_CALL_TYPES.as_ptr(),
        import_node_types: JS_IMPORT_TYPES.as_ptr(),
        import_from_types: JS_IMPORT_TYPES.as_ptr(),
        branching_node_types: JS_BRANCH_TYPES.as_ptr(),
        variable_node_types: JS_VAR_TYPES.as_ptr(),
        assignment_node_types: JS_ASSIGN_TYPES.as_ptr(),
        throw_node_types: JS_THROW_TYPES.as_ptr(),
        decorator_node_types: TS_DECORATOR_TYPES.as_ptr(),
        env_access_member_patterns: TS_ENV_MEMBERS.as_ptr(),
        ts_factory: Some(tree_sitter_tsx),
        ..LIST_DEFAULTS
    };
    specs[lang::RUST as usize] = CbmLangSpec {
        language: lang::RUST,
        function_node_types: RUST_FUNC_TYPES.as_ptr(),
        class_node_types: RUST_CLASS_TYPES.as_ptr(),
        field_node_types: RUST_FIELD_TYPES.as_ptr(),
        module_node_types: RUST_MODULE_TYPES.as_ptr(),
        call_node_types: RUST_CALL_TYPES.as_ptr(),
        import_node_types: RUST_IMPORT_TYPES.as_ptr(),
        import_from_types: RUST_IMPORT_FROM_TYPES.as_ptr(),
        branching_node_types: RUST_BRANCH_TYPES.as_ptr(),
        variable_node_types: RUST_VAR_TYPES.as_ptr(),
        assignment_node_types: RUST_ASSIGN_TYPES.as_ptr(),
        decorator_node_types: RUST_DECORATOR_TYPES.as_ptr(),
        env_access_functions: RUST_ENV_FUNCS.as_ptr(),
        ts_factory: Some(tree_sitter_rust),
        ..LIST_DEFAULTS
    };
    specs[lang::JAVA as usize] = CbmLangSpec {
        language: lang::JAVA,
        function_node_types: JAVA_FUNC_TYPES.as_ptr(),
        class_node_types: JAVA_CLASS_TYPES.as_ptr(),
        field_node_types: JAVA_FIELD_TYPES.as_ptr(),
        module_node_types: JAVA_MODULE_TYPES.as_ptr(),
        call_node_types: JAVA_CALL_TYPES.as_ptr(),
        import_node_types: JAVA_IMPORT_TYPES.as_ptr(),
        import_from_types: JAVA_IMPORT_TYPES.as_ptr(),
        branching_node_types: JAVA_BRANCH_TYPES.as_ptr(),
        variable_node_types: JAVA_VAR_TYPES.as_ptr(),
        assignment_node_types: JAVA_ASSIGN_TYPES.as_ptr(),
        throw_node_types: JAVA_THROW_TYPES.as_ptr(),
        throws_clause_field: c"throws".as_ptr(),
        decorator_node_types: JAVA_DECORATOR_TYPES.as_ptr(),
        env_access_functions: JAVA_ENV_FUNCS.as_ptr(),
        ts_factory: Some(tree_sitter_java),
        ..LIST_DEFAULTS
    };
    specs[lang::CPP as usize] = CbmLangSpec {
        language: lang::CPP,
        function_node_types: CPP_FUNC_TYPES.as_ptr(),
        class_node_types: CPP_CLASS_TYPES.as_ptr(),
        field_node_types: CPP_FIELD_TYPES.as_ptr(),
        module_node_types: CPP_MODULE_TYPES.as_ptr(),
        call_node_types: CPP_CALL_TYPES.as_ptr(),
        import_node_types: CPP_IMPORT_TYPES.as_ptr(),
        import_from_types: CPP_IMPORT_TYPES.as_ptr(),
        branching_node_types: CPP_BRANCH_TYPES.as_ptr(),
        variable_node_types: CPP_VAR_TYPES.as_ptr(),
        assignment_node_types: CPP_ASSIGN_TYPES.as_ptr(),
        throw_node_types: CPP_THROW_TYPES.as_ptr(),
        env_access_functions: CPP_ENV_FUNCS.as_ptr(),
        ts_factory: Some(tree_sitter_cpp),
        ..LIST_DEFAULTS
    };
    specs[lang::CSHARP as usize] = CbmLangSpec {
        language: lang::CSHARP,
        function_node_types: CS_FUNC_TYPES.as_ptr(),
        class_node_types: CS_CLASS_TYPES.as_ptr(),
        field_node_types: CS_FIELD_TYPES.as_ptr(),
        module_node_types: CS_MODULE_TYPES.as_ptr(),
        call_node_types: CS_CALL_TYPES.as_ptr(),
        import_node_types: CS_IMPORT_TYPES.as_ptr(),
        import_from_types: CS_IMPORT_TYPES.as_ptr(),
        branching_node_types: CS_BRANCH_TYPES.as_ptr(),
        variable_node_types: CS_VAR_TYPES.as_ptr(),
        assignment_node_types: CS_ASSIGN_TYPES.as_ptr(),
        throw_node_types: CS_THROW_TYPES.as_ptr(),
        decorator_node_types: CS_DECORATOR_TYPES.as_ptr(),
        env_access_functions: CS_ENV_FUNCS.as_ptr(),
        ts_factory: Some(tree_sitter_c_sharp),
        ..LIST_DEFAULTS
    };
    specs[lang::PHP as usize] = CbmLangSpec {
        language: lang::PHP,
        function_node_types: PHP_FUNC_TYPES.as_ptr(),
        class_node_types: PHP_CLASS_TYPES.as_ptr(),
        module_node_types: PHP_MODULE_TYPES.as_ptr(),
        call_node_types: PHP_CALL_TYPES.as_ptr(),
        import_node_types: PHP_IMPORT_TYPES.as_ptr(),
        branching_node_types: PHP_BRANCH_TYPES.as_ptr(),
        variable_node_types: PHP_VAR_TYPES.as_ptr(),
        assignment_node_types: PHP_ASSIGN_TYPES.as_ptr(),
        throw_node_types: PHP_THROW_TYPES.as_ptr(),
        decorator_node_types: PHP_DECORATOR_TYPES.as_ptr(),
        env_access_functions: PHP_ENV_FUNCS.as_ptr(),
        ts_factory: Some(tree_sitter_php_only),
        ..LIST_DEFAULTS
    };
    specs[lang::LUA as usize] = CbmLangSpec {
        language: lang::LUA,
        function_node_types: LUA_FUNC_TYPES.as_ptr(),
        module_node_types: LUA_MODULE_TYPES.as_ptr(),
        call_node_types: LUA_CALL_TYPES.as_ptr(),
        import_node_types: LUA_IMPORT_TYPES.as_ptr(),
        branching_node_types: LUA_BRANCH_TYPES.as_ptr(),
        variable_node_types: LUA_VAR_TYPES.as_ptr(),
        assignment_node_types: LUA_ASSIGN_TYPES.as_ptr(),
        env_access_functions: LUA_ENV_FUNCS.as_ptr(),
        ts_factory: Some(tree_sitter_lua),
        ..LIST_DEFAULTS
    };
    specs[lang::SCALA as usize] = CbmLangSpec {
        language: lang::SCALA,
        function_node_types: SCALA_FUNC_TYPES.as_ptr(),
        class_node_types: SCALA_CLASS_TYPES.as_ptr(),
        module_node_types: SCALA_MODULE_TYPES.as_ptr(),
        call_node_types: SCALA_CALL_TYPES.as_ptr(),
        import_node_types: SCALA_IMPORT_TYPES.as_ptr(),
        import_from_types: SCALA_IMPORT_TYPES.as_ptr(),
        branching_node_types: SCALA_BRANCH_TYPES.as_ptr(),
        variable_node_types: SCALA_VAR_TYPES.as_ptr(),
        assignment_node_types: SCALA_ASSIGN_TYPES.as_ptr(),
        throw_node_types: SCALA_THROW_TYPES.as_ptr(),
        decorator_node_types: SCALA_DECORATOR_TYPES.as_ptr(),
        env_access_functions: SCALA_ENV_FUNCS.as_ptr(),
        ts_factory: Some(tree_sitter_scala),
        ..LIST_DEFAULTS
    };
    specs[lang::KOTLIN as usize] = CbmLangSpec {
        language: lang::KOTLIN,
        function_node_types: KOTLIN_FUNC_TYPES.as_ptr(),
        class_node_types: KOTLIN_CLASS_TYPES.as_ptr(),
        module_node_types: KOTLIN_MODULE_TYPES.as_ptr(),
        call_node_types: KOTLIN_CALL_TYPES.as_ptr(),
        import_node_types: KOTLIN_IMPORT_TYPES.as_ptr(),
        import_from_types: KOTLIN_IMPORT_TYPES.as_ptr(),
        branching_node_types: KOTLIN_BRANCH_TYPES.as_ptr(),
        variable_node_types: KOTLIN_VAR_TYPES.as_ptr(),
        assignment_node_types: KOTLIN_ASSIGN_TYPES.as_ptr(),
        throw_node_types: KOTLIN_THROW_TYPES.as_ptr(),
        decorator_node_types: KOTLIN_DECORATOR_TYPES.as_ptr(),
        env_access_functions: KOTLIN_ENV_FUNCS.as_ptr(),
        ts_factory: Some(tree_sitter_kotlin),
        ..LIST_DEFAULTS
    };
    specs[lang::RUBY as usize] = CbmLangSpec {
        language: lang::RUBY,
        function_node_types: RUBY_FUNC_TYPES.as_ptr(),
        class_node_types: RUBY_CLASS_TYPES.as_ptr(),
        module_node_types: RUBY_MODULE_TYPES.as_ptr(),
        call_node_types: RUBY_CALL_TYPES.as_ptr(),
        import_node_types: RUBY_IMPORT_TYPES.as_ptr(),
        branching_node_types: RUBY_BRANCH_TYPES.as_ptr(),
        variable_node_types: RUBY_VAR_TYPES.as_ptr(),
        assignment_node_types: RUBY_ASSIGN_TYPES.as_ptr(),
        env_access_member_patterns: RUBY_ENV_MEMBERS.as_ptr(),
        ts_factory: Some(tree_sitter_ruby),
        ..LIST_DEFAULTS
    };
    specs[lang::C as usize] = CbmLangSpec {
        language: lang::C,
        function_node_types: C_FUNC_TYPES.as_ptr(),
        class_node_types: C_CLASS_TYPES.as_ptr(),
        field_node_types: C_FIELD_TYPES.as_ptr(),
        module_node_types: C_MODULE_TYPES.as_ptr(),
        call_node_types: C_CALL_TYPES.as_ptr(),
        import_node_types: C_IMPORT_TYPES.as_ptr(),
        branching_node_types: C_BRANCH_TYPES.as_ptr(),
        variable_node_types: C_VAR_TYPES.as_ptr(),
        assignment_node_types: C_ASSIGN_TYPES.as_ptr(),
        env_access_functions: C_ENV_FUNCS.as_ptr(),
        ts_factory: Some(tree_sitter_c),
        ..LIST_DEFAULTS
    };
    specs[lang::BASH as usize] = CbmLangSpec {
        language: lang::BASH,
        function_node_types: BASH_FUNC_TYPES.as_ptr(),
        module_node_types: BASH_MODULE_TYPES.as_ptr(),
        call_node_types: BASH_CALL_TYPES.as_ptr(),
        import_node_types: BASH_IMPORT_TYPES.as_ptr(),
        branching_node_types: BASH_BRANCH_TYPES.as_ptr(),
        variable_node_types: BASH_VAR_TYPES.as_ptr(),
        assignment_node_types: BASH_VAR_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_bash),
        ..LIST_DEFAULTS
    };
    specs[lang::ZIG as usize] = CbmLangSpec {
        language: lang::ZIG,
        function_node_types: ZIG_FUNC_TYPES.as_ptr(),
        class_node_types: ZIG_CLASS_TYPES.as_ptr(),
        field_node_types: ZIG_FIELD_TYPES.as_ptr(),
        module_node_types: ZIG_MODULE_TYPES.as_ptr(),
        call_node_types: ZIG_CALL_TYPES.as_ptr(),
        import_node_types: ZIG_IMPORT_TYPES.as_ptr(),
        branching_node_types: ZIG_BRANCH_TYPES.as_ptr(),
        variable_node_types: ZIG_VAR_TYPES.as_ptr(),
        assignment_node_types: ZIG_ASSIGN_TYPES.as_ptr(),
        env_access_functions: ZIG_ENV_FUNCS.as_ptr(),
        ts_factory: Some(tree_sitter_zig),
        ..LIST_DEFAULTS
    };
    specs[lang::ELIXIR as usize] = CbmLangSpec {
        language: lang::ELIXIR,
        function_node_types: ELIXIR_FUNC_TYPES.as_ptr(),
        module_node_types: ELIXIR_MODULE_TYPES.as_ptr(),
        call_node_types: ELIXIR_CALL_TYPES.as_ptr(),
        import_node_types: ELIXIR_IMPORT_TYPES.as_ptr(),
        branching_node_types: ELIXIR_BRANCH_TYPES.as_ptr(),
        variable_node_types: ELIXIR_VAR_TYPES.as_ptr(),
        assignment_node_types: ELIXIR_VAR_TYPES.as_ptr(),
        env_access_functions: ELIXIR_ENV_FUNCS.as_ptr(),
        ts_factory: Some(tree_sitter_elixir),
        ..LIST_DEFAULTS
    };
    specs[lang::HASKELL as usize] = CbmLangSpec {
        language: lang::HASKELL,
        function_node_types: HASKELL_FUNC_TYPES.as_ptr(),
        class_node_types: HASKELL_CLASS_TYPES.as_ptr(),
        module_node_types: HASKELL_MODULE_TYPES.as_ptr(),
        call_node_types: HASKELL_CALL_TYPES.as_ptr(),
        import_node_types: HASKELL_IMPORT_TYPES.as_ptr(),
        branching_node_types: HASKELL_BRANCH_TYPES.as_ptr(),
        variable_node_types: HASKELL_VAR_TYPES.as_ptr(),
        assignment_node_types: HASKELL_VAR_TYPES.as_ptr(),
        env_access_functions: HASKELL_ENV_FUNCS.as_ptr(),
        ts_factory: Some(tree_sitter_haskell),
        ..LIST_DEFAULTS
    };
    specs[lang::OCAML as usize] = CbmLangSpec {
        language: lang::OCAML,
        function_node_types: OCAML_FUNC_TYPES.as_ptr(),
        class_node_types: OCAML_CLASS_TYPES.as_ptr(),
        module_node_types: OCAML_MODULE_TYPES.as_ptr(),
        call_node_types: OCAML_CALL_TYPES.as_ptr(),
        import_node_types: OCAML_IMPORT_TYPES.as_ptr(),
        branching_node_types: OCAML_BRANCH_TYPES.as_ptr(),
        variable_node_types: OCAML_VAR_TYPES.as_ptr(),
        assignment_node_types: OCAML_VAR_TYPES.as_ptr(),
        env_access_functions: OCAML_ENV_FUNCS.as_ptr(),
        ts_factory: Some(tree_sitter_ocaml),
        ..LIST_DEFAULTS
    };
    specs[lang::OBJC as usize] = CbmLangSpec {
        language: lang::OBJC,
        function_node_types: OBJC_FUNC_TYPES.as_ptr(),
        class_node_types: OBJC_CLASS_TYPES.as_ptr(),
        field_node_types: OBJC_FIELD_TYPES.as_ptr(),
        module_node_types: OBJC_MODULE_TYPES.as_ptr(),
        call_node_types: OBJC_CALL_TYPES.as_ptr(),
        import_node_types: OBJC_IMPORT_TYPES.as_ptr(),
        branching_node_types: OBJC_BRANCH_TYPES.as_ptr(),
        variable_node_types: OBJC_VAR_TYPES.as_ptr(),
        assignment_node_types: OBJC_ASSIGN_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_objc),
        ..LIST_DEFAULTS
    };
    specs[lang::SWIFT as usize] = CbmLangSpec {
        language: lang::SWIFT,
        function_node_types: SWIFT_FUNC_TYPES.as_ptr(),
        class_node_types: SWIFT_CLASS_TYPES.as_ptr(),
        field_node_types: SWIFT_FIELD_TYPES.as_ptr(),
        module_node_types: SWIFT_MODULE_TYPES.as_ptr(),
        call_node_types: SWIFT_CALL_TYPES.as_ptr(),
        import_node_types: SWIFT_IMPORT_TYPES.as_ptr(),
        branching_node_types: SWIFT_BRANCH_TYPES.as_ptr(),
        variable_node_types: SWIFT_VAR_TYPES.as_ptr(),
        assignment_node_types: SWIFT_ASSIGN_TYPES.as_ptr(),
        throw_node_types: SWIFT_THROW_TYPES.as_ptr(),
        decorator_node_types: SWIFT_DECORATOR_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_swift),
        ..LIST_DEFAULTS
    };
    specs[lang::DART as usize] = CbmLangSpec {
        language: lang::DART,
        function_node_types: DART_FUNC_TYPES.as_ptr(),
        class_node_types: DART_CLASS_TYPES.as_ptr(),
        field_node_types: DART_FIELD_TYPES.as_ptr(),
        module_node_types: DART_MODULE_TYPES.as_ptr(),
        call_node_types: DART_CALL_TYPES.as_ptr(),
        import_node_types: DART_IMPORT_TYPES.as_ptr(),
        branching_node_types: DART_BRANCH_TYPES.as_ptr(),
        variable_node_types: DART_VAR_TYPES.as_ptr(),
        assignment_node_types: DART_ASSIGN_TYPES.as_ptr(),
        throw_node_types: DART_THROW_TYPES.as_ptr(),
        decorator_node_types: DART_DECORATOR_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_dart),
        ..LIST_DEFAULTS
    };
    specs[lang::PERL as usize] = CbmLangSpec {
        language: lang::PERL,
        function_node_types: PERL_FUNC_TYPES.as_ptr(),
        module_node_types: PERL_MODULE_TYPES.as_ptr(),
        call_node_types: PERL_CALL_TYPES.as_ptr(),
        import_node_types: PERL_IMPORT_TYPES.as_ptr(),
        branching_node_types: PERL_BRANCH_TYPES.as_ptr(),
        variable_node_types: PERL_VAR_TYPES.as_ptr(),
        assignment_node_types: PERL_ASSIGN_TYPES.as_ptr(),
        env_access_functions: PERL_ENV_FUNCS.as_ptr(),
        ts_factory: Some(tree_sitter_perl),
        ..LIST_DEFAULTS
    };
    specs[lang::GROOVY as usize] = CbmLangSpec {
        language: lang::GROOVY,
        function_node_types: GROOVY_FUNC_TYPES.as_ptr(),
        class_node_types: GROOVY_CLASS_TYPES.as_ptr(),
        module_node_types: GROOVY_MODULE_TYPES.as_ptr(),
        call_node_types: GROOVY_CALL_TYPES.as_ptr(),
        import_node_types: GROOVY_IMPORT_TYPES.as_ptr(),
        branching_node_types: GROOVY_BRANCH_TYPES.as_ptr(),
        variable_node_types: GROOVY_VAR_TYPES.as_ptr(),
        assignment_node_types: GROOVY_ASSIGN_TYPES.as_ptr(),
        throw_node_types: GROOVY_THROW_TYPES.as_ptr(),
        decorator_node_types: GROOVY_DECORATOR_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_groovy),
        ..LIST_DEFAULTS
    };
    specs[lang::ERLANG as usize] = CbmLangSpec {
        language: lang::ERLANG,
        function_node_types: ERLANG_FUNC_TYPES.as_ptr(),
        class_node_types: ERLANG_CLASS_TYPES.as_ptr(),
        module_node_types: ERLANG_MODULE_TYPES.as_ptr(),
        call_node_types: ERLANG_CALL_TYPES.as_ptr(),
        import_node_types: ERLANG_IMPORT_TYPES.as_ptr(),
        branching_node_types: ERLANG_BRANCH_TYPES.as_ptr(),
        variable_node_types: ERLANG_VAR_TYPES.as_ptr(),
        assignment_node_types: ERLANG_ASSIGN_TYPES.as_ptr(),
        throw_node_types: ERLANG_THROW_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_erlang),
        ..LIST_DEFAULTS
    };
    specs[lang::R as usize] = CbmLangSpec {
        language: lang::R,
        function_node_types: R_FUNC_TYPES.as_ptr(),
        module_node_types: R_MODULE_TYPES.as_ptr(),
        call_node_types: R_CALL_TYPES.as_ptr(),
        import_node_types: R_IMPORT_TYPES.as_ptr(),
        branching_node_types: R_BRANCH_TYPES.as_ptr(),
        variable_node_types: R_VAR_TYPES.as_ptr(),
        assignment_node_types: R_VAR_TYPES.as_ptr(),
        env_access_functions: R_ENV_FUNCS.as_ptr(),
        ts_factory: Some(tree_sitter_r),
        ..LIST_DEFAULTS
    };
    specs[lang::HTML as usize] = CbmLangSpec {
        language: lang::HTML,
        module_node_types: HTML_MODULE_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_html),
        embedded_imports: HTML_EMBEDDED_IMPORTS.as_ptr(),
        ..LIST_DEFAULTS
    };
    specs[lang::CSS as usize] = CbmLangSpec {
        language: lang::CSS,
        module_node_types: CSS_MODULE_TYPES.as_ptr(),
        call_node_types: CSS_CALL_TYPES.as_ptr(),
        import_node_types: CSS_IMPORT_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_css),
        ..LIST_DEFAULTS
    };
    specs[lang::SCSS as usize] = CbmLangSpec {
        language: lang::SCSS,
        function_node_types: SCSS_FUNC_TYPES.as_ptr(),
        module_node_types: SCSS_MODULE_TYPES.as_ptr(),
        call_node_types: SCSS_CALL_TYPES.as_ptr(),
        import_node_types: SCSS_IMPORT_TYPES.as_ptr(),
        branching_node_types: SCSS_BRANCH_TYPES.as_ptr(),
        variable_node_types: SCSS_VAR_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_scss),
        ..LIST_DEFAULTS
    };
    specs[lang::YAML as usize] = CbmLangSpec {
        language: lang::YAML,
        module_node_types: YAML_MODULE_TYPES.as_ptr(),
        variable_node_types: YAML_VAR_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_yaml),
        ..LIST_DEFAULTS
    };
    specs[lang::TOML as usize] = CbmLangSpec {
        language: lang::TOML,
        class_node_types: TOML_CLASS_TYPES.as_ptr(),
        module_node_types: TOML_MODULE_TYPES.as_ptr(),
        variable_node_types: TOML_VAR_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_toml),
        ..LIST_DEFAULTS
    };
    specs[lang::HCL as usize] = CbmLangSpec {
        language: lang::HCL,
        class_node_types: HCL_CLASS_TYPES.as_ptr(),
        module_node_types: HCL_MODULE_TYPES.as_ptr(),
        call_node_types: HCL_CALL_TYPES.as_ptr(),
        variable_node_types: HCL_VAR_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_hcl),
        ..LIST_DEFAULTS
    };
    specs[lang::SQL as usize] = CbmLangSpec {
        language: lang::SQL,
        function_node_types: SQL_FUNC_TYPES.as_ptr(),
        class_node_types: SQL_CLASS_TYPES.as_ptr(),
        field_node_types: SQL_FIELD_TYPES.as_ptr(),
        module_node_types: SQL_MODULE_TYPES.as_ptr(),
        call_node_types: SQL_CALL_TYPES.as_ptr(),
        branching_node_types: SQL_BRANCH_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_sql),
        ..LIST_DEFAULTS
    };
    specs[lang::DOCKERFILE as usize] = CbmLangSpec {
        language: lang::DOCKERFILE,
        module_node_types: DOCKERFILE_MODULE_TYPES.as_ptr(),
        variable_node_types: DOCKERFILE_VAR_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_dockerfile),
        ..LIST_DEFAULTS
    };
    specs[lang::CLOJURE as usize] = CbmLangSpec {
        language: lang::CLOJURE,
        function_node_types: CLOJURE_FUNC_TYPES.as_ptr(),
        module_node_types: CLOJURE_MODULE_TYPES.as_ptr(),
        call_node_types: CLOJURE_CALL_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_clojure),
        ..LIST_DEFAULTS
    };
    specs[lang::FSHARP as usize] = CbmLangSpec {
        language: lang::FSHARP,
        function_node_types: FSHARP_FUNC_TYPES.as_ptr(),
        class_node_types: FSHARP_CLASS_TYPES.as_ptr(),
        module_node_types: FSHARP_MODULE_TYPES.as_ptr(),
        call_node_types: FSHARP_CALL_TYPES.as_ptr(),
        import_node_types: FSHARP_IMPORT_TYPES.as_ptr(),
        branching_node_types: FSHARP_BRANCH_TYPES.as_ptr(),
        variable_node_types: FSHARP_VAR_TYPES.as_ptr(),
        assignment_node_types: FSHARP_VAR_TYPES.as_ptr(),
        env_access_functions: FSHARP_ENV_FUNCS.as_ptr(),
        ts_factory: Some(tree_sitter_fsharp),
        ..LIST_DEFAULTS
    };
    specs[lang::JULIA as usize] = CbmLangSpec {
        language: lang::JULIA,
        function_node_types: JULIA_FUNC_TYPES.as_ptr(),
        class_node_types: JULIA_CLASS_TYPES.as_ptr(),
        module_node_types: JULIA_MODULE_TYPES.as_ptr(),
        call_node_types: JULIA_CALL_TYPES.as_ptr(),
        import_node_types: JULIA_IMPORT_TYPES.as_ptr(),
        branching_node_types: JULIA_BRANCH_TYPES.as_ptr(),
        variable_node_types: JULIA_VAR_TYPES.as_ptr(),
        assignment_node_types: JULIA_ASSIGN_TYPES.as_ptr(),
        throw_node_types: JULIA_THROW_TYPES.as_ptr(),
        env_access_functions: JULIA_ENV_FUNCS.as_ptr(),
        ts_factory: Some(tree_sitter_julia),
        ..LIST_DEFAULTS
    };
    specs[lang::VIMSCRIPT as usize] = CbmLangSpec {
        language: lang::VIMSCRIPT,
        function_node_types: VIM_FUNC_TYPES.as_ptr(),
        module_node_types: VIM_MODULE_TYPES.as_ptr(),
        call_node_types: VIM_CALL_TYPES.as_ptr(),
        import_node_types: VIM_IMPORT_TYPES.as_ptr(),
        branching_node_types: VIM_BRANCH_TYPES.as_ptr(),
        variable_node_types: VIM_VAR_TYPES.as_ptr(),
        assignment_node_types: VIM_VAR_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_vim),
        ..LIST_DEFAULTS
    };
    specs[lang::NIX as usize] = CbmLangSpec {
        language: lang::NIX,
        function_node_types: NIX_FUNC_TYPES.as_ptr(),
        module_node_types: NIX_MODULE_TYPES.as_ptr(),
        call_node_types: NIX_CALL_TYPES.as_ptr(),
        branching_node_types: NIX_BRANCH_TYPES.as_ptr(),
        variable_node_types: NIX_VAR_TYPES.as_ptr(),
        assignment_node_types: NIX_VAR_TYPES.as_ptr(),
        env_access_functions: NIX_ENV_FUNCS.as_ptr(),
        ts_factory: Some(tree_sitter_nix),
        ..LIST_DEFAULTS
    };
    specs[lang::COMMONLISP as usize] = CbmLangSpec {
        language: lang::COMMONLISP,
        function_node_types: COMMONLISP_FUNC_TYPES.as_ptr(),
        module_node_types: COMMONLISP_MODULE_TYPES.as_ptr(),
        call_node_types: COMMONLISP_CALL_TYPES.as_ptr(),
        import_node_types: COMMONLISP_IMPORT_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_commonlisp),
        ..LIST_DEFAULTS
    };
    specs[lang::ELM as usize] = CbmLangSpec {
        language: lang::ELM,
        function_node_types: ELM_FUNC_TYPES.as_ptr(),
        class_node_types: ELM_CLASS_TYPES.as_ptr(),
        module_node_types: ELM_MODULE_TYPES.as_ptr(),
        call_node_types: ELM_CALL_TYPES.as_ptr(),
        import_node_types: ELM_IMPORT_TYPES.as_ptr(),
        branching_node_types: ELM_BRANCH_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_elm),
        ..LIST_DEFAULTS
    };
    specs[lang::FORTRAN as usize] = CbmLangSpec {
        language: lang::FORTRAN,
        function_node_types: FORTRAN_FUNC_TYPES.as_ptr(),
        class_node_types: FORTRAN_CLASS_TYPES.as_ptr(),
        module_node_types: FORTRAN_MODULE_TYPES.as_ptr(),
        call_node_types: FORTRAN_CALL_TYPES.as_ptr(),
        import_node_types: FORTRAN_IMPORT_TYPES.as_ptr(),
        branching_node_types: FORTRAN_BRANCH_TYPES.as_ptr(),
        variable_node_types: FORTRAN_VAR_TYPES.as_ptr(),
        assignment_node_types: FORTRAN_ASSIGN_TYPES.as_ptr(),
        env_access_functions: FORTRAN_ENV_FUNCS.as_ptr(),
        ts_factory: Some(tree_sitter_fortran),
        ..LIST_DEFAULTS
    };

    // CBM_LANG_CUDA (reuses C++ node types)
    specs[lang::CUDA as usize] = CbmLangSpec {
        language: lang::CUDA,
        function_node_types: CPP_FUNC_TYPES.as_ptr(),
        class_node_types: CPP_CLASS_TYPES.as_ptr(),
        field_node_types: CPP_FIELD_TYPES.as_ptr(),
        module_node_types: CPP_MODULE_TYPES.as_ptr(),
        call_node_types: CPP_CALL_TYPES.as_ptr(),
        import_node_types: CPP_IMPORT_TYPES.as_ptr(),
        import_from_types: CPP_IMPORT_TYPES.as_ptr(),
        branching_node_types: CPP_BRANCH_TYPES.as_ptr(),
        variable_node_types: CPP_VAR_TYPES.as_ptr(),
        assignment_node_types: CPP_ASSIGN_TYPES.as_ptr(),
        throw_node_types: CPP_THROW_TYPES.as_ptr(),
        env_access_functions: CPP_ENV_FUNCS.as_ptr(),
        ts_factory: Some(tree_sitter_cuda),
        ..LIST_DEFAULTS
    };
    specs[lang::COBOL as usize] = CbmLangSpec {
        language: lang::COBOL,
        function_node_types: COBOL_FUNC_TYPES.as_ptr(),
        module_node_types: COBOL_MODULE_TYPES.as_ptr(),
        call_node_types: COBOL_CALL_TYPES.as_ptr(),
        import_node_types: COBOL_IMPORT_TYPES.as_ptr(),
        branching_node_types: COBOL_BRANCH_TYPES.as_ptr(),
        variable_node_types: COBOL_VAR_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_COBOL),
        ..LIST_DEFAULTS
    };
    specs[lang::VERILOG as usize] = CbmLangSpec {
        language: lang::VERILOG,
        function_node_types: VERILOG_FUNC_TYPES.as_ptr(),
        class_node_types: VERILOG_CLASS_TYPES.as_ptr(),
        module_node_types: VERILOG_MODULE_TYPES.as_ptr(),
        call_node_types: VERILOG_CALL_TYPES.as_ptr(),
        import_node_types: VERILOG_IMPORT_TYPES.as_ptr(),
        branching_node_types: VERILOG_BRANCH_TYPES.as_ptr(),
        variable_node_types: VERILOG_VAR_TYPES.as_ptr(),
        assignment_node_types: VERILOG_ASSIGN_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_verilog),
        ..LIST_DEFAULTS
    };
    specs[lang::EMACSLISP as usize] = CbmLangSpec {
        language: lang::EMACSLISP,
        function_node_types: ELISP_FUNC_TYPES.as_ptr(),
        module_node_types: ELISP_MODULE_TYPES.as_ptr(),
        call_node_types: ELISP_CALL_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_elisp),
        ..LIST_DEFAULTS
    };
    specs[lang::JSON as usize] = CbmLangSpec {
        language: lang::JSON,
        module_node_types: JSON_MODULE_TYPES.as_ptr(),
        variable_node_types: JSON_VAR_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_json),
        ..LIST_DEFAULTS
    };
    specs[lang::XML as usize] = CbmLangSpec {
        language: lang::XML,
        class_node_types: XML_CLASS_TYPES.as_ptr(),
        module_node_types: XML_MODULE_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_xml),
        ..LIST_DEFAULTS
    };
    specs[lang::MARKDOWN as usize] = CbmLangSpec {
        language: lang::MARKDOWN,
        class_node_types: MARKDOWN_CLASS_TYPES.as_ptr(),
        module_node_types: MARKDOWN_MODULE_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_markdown),
        ..LIST_DEFAULTS
    };
    specs[lang::MAKEFILE as usize] = CbmLangSpec {
        language: lang::MAKEFILE,
        function_node_types: MAKEFILE_FUNC_TYPES.as_ptr(),
        module_node_types: MAKEFILE_MODULE_TYPES.as_ptr(),
        call_node_types: MAKEFILE_CALL_TYPES.as_ptr(),
        import_node_types: MAKEFILE_IMPORT_TYPES.as_ptr(),
        variable_node_types: MAKEFILE_VAR_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_make),
        ..LIST_DEFAULTS
    };
    specs[lang::CMAKE as usize] = CbmLangSpec {
        language: lang::CMAKE,
        function_node_types: CMAKE_FUNC_TYPES.as_ptr(),
        module_node_types: CMAKE_MODULE_TYPES.as_ptr(),
        call_node_types: CMAKE_CALL_TYPES.as_ptr(),
        import_node_types: MAKE_IMPORT_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_cmake),
        ..LIST_DEFAULTS
    };
    specs[lang::PROTOBUF as usize] = CbmLangSpec {
        language: lang::PROTOBUF,
        function_node_types: PROTOBUF_FUNC_TYPES.as_ptr(),
        class_node_types: PROTOBUF_CLASS_TYPES.as_ptr(),
        field_node_types: PROTOBUF_FIELD_TYPES.as_ptr(),
        module_node_types: PROTOBUF_MODULE_TYPES.as_ptr(),
        import_node_types: PROTOBUF_IMPORT_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_proto),
        ..LIST_DEFAULTS
    };
    specs[lang::GRAPHQL as usize] = CbmLangSpec {
        language: lang::GRAPHQL,
        class_node_types: GRAPHQL_CLASS_TYPES.as_ptr(),
        field_node_types: GRAPHQL_FIELD_TYPES.as_ptr(),
        module_node_types: GRAPHQL_MODULE_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_graphql),
        ..LIST_DEFAULTS
    };
    specs[lang::VUE as usize] = CbmLangSpec {
        language: lang::VUE,
        module_node_types: VUE_MODULE_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_vue),
        embedded_imports: VUE_EMBEDDED_IMPORTS.as_ptr(),
        ..LIST_DEFAULTS
    };
    specs[lang::SVELTE as usize] = CbmLangSpec {
        language: lang::SVELTE,
        module_node_types: SVELTE_MODULE_TYPES.as_ptr(),
        branching_node_types: SVELTE_BRANCH_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_svelte),
        embedded_imports: SVELTE_EMBEDDED_IMPORTS.as_ptr(),
        ..LIST_DEFAULTS
    };
    specs[lang::MESON as usize] = CbmLangSpec {
        language: lang::MESON,
        function_node_types: MESON_FUNC_TYPES.as_ptr(),
        module_node_types: MESON_MODULE_TYPES.as_ptr(),
        call_node_types: MESON_CALL_TYPES.as_ptr(),
        branching_node_types: MESON_BRANCH_TYPES.as_ptr(),
        variable_node_types: MESON_VAR_TYPES.as_ptr(),
        assignment_node_types: MESON_VAR_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_meson),
        ..LIST_DEFAULTS
    };

    // CBM_LANG_GLSL (reuses C node types)
    specs[lang::GLSL as usize] = CbmLangSpec {
        language: lang::GLSL,
        function_node_types: C_FUNC_TYPES.as_ptr(),
        class_node_types: C_CLASS_TYPES.as_ptr(),
        field_node_types: C_FIELD_TYPES.as_ptr(),
        module_node_types: C_MODULE_TYPES.as_ptr(),
        call_node_types: C_CALL_TYPES.as_ptr(),
        import_node_types: C_IMPORT_TYPES.as_ptr(),
        branching_node_types: C_BRANCH_TYPES.as_ptr(),
        variable_node_types: C_VAR_TYPES.as_ptr(),
        assignment_node_types: C_ASSIGN_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_glsl),
        ..LIST_DEFAULTS
    };
    specs[lang::INI as usize] = CbmLangSpec {
        language: lang::INI,
        class_node_types: INI_CLASS_TYPES.as_ptr(),
        module_node_types: INI_MODULE_TYPES.as_ptr(),
        variable_node_types: INI_VAR_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_ini),
        ..LIST_DEFAULTS
    };
    specs[lang::MATLAB as usize] = CbmLangSpec {
        language: lang::MATLAB,
        function_node_types: MATLAB_FUNC_TYPES.as_ptr(),
        class_node_types: MATLAB_CLASS_TYPES.as_ptr(),
        module_node_types: MATLAB_MODULE_TYPES.as_ptr(),
        call_node_types: MATLAB_CALL_TYPES.as_ptr(),
        branching_node_types: MATLAB_BRANCH_TYPES.as_ptr(),
        variable_node_types: MATLAB_VAR_TYPES.as_ptr(),
        assignment_node_types: MATLAB_VAR_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_matlab),
        ..LIST_DEFAULTS
    };
    specs[lang::LEAN as usize] = CbmLangSpec {
        language: lang::LEAN,
        function_node_types: LEAN_FUNC_TYPES.as_ptr(),
        class_node_types: LEAN_CLASS_TYPES.as_ptr(),
        module_node_types: LEAN_MODULE_TYPES.as_ptr(),
        call_node_types: LEAN_CALL_TYPES.as_ptr(),
        import_node_types: LEAN_IMPORT_TYPES.as_ptr(),
        branching_node_types: LEAN_BRANCH_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_lean),
        ..LIST_DEFAULTS
    };
    specs[lang::FORM as usize] = CbmLangSpec {
        language: lang::FORM,
        function_node_types: FORM_FUNC_TYPES.as_ptr(),
        module_node_types: FORM_MODULE_TYPES.as_ptr(),
        call_node_types: FORM_CALL_TYPES.as_ptr(),
        import_node_types: FORM_IMPORT_TYPES.as_ptr(),
        branching_node_types: FORM_BRANCH_TYPES.as_ptr(),
        variable_node_types: FORM_VAR_TYPES.as_ptr(),
        assignment_node_types: FORM_ASSIGN_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_form),
        ..LIST_DEFAULTS
    };
    specs[lang::MAGMA as usize] = CbmLangSpec {
        language: lang::MAGMA,
        function_node_types: MAGMA_FUNC_TYPES.as_ptr(),
        module_node_types: MAGMA_MODULE_TYPES.as_ptr(),
        call_node_types: MAGMA_CALL_TYPES.as_ptr(),
        import_node_types: MAGMA_IMPORT_TYPES.as_ptr(),
        branching_node_types: MAGMA_BRANCH_TYPES.as_ptr(),
        variable_node_types: MAGMA_VAR_TYPES.as_ptr(),
        assignment_node_types: MAGMA_VAR_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_magma),
        ..LIST_DEFAULTS
    };
    specs[lang::WOLFRAM as usize] = CbmLangSpec {
        language: lang::WOLFRAM,
        function_node_types: WOLFRAM_FUNC_TYPES.as_ptr(),
        module_node_types: WOLFRAM_MODULE_TYPES.as_ptr(),
        call_node_types: WOLFRAM_CALL_TYPES.as_ptr(),
        import_node_types: WOLFRAM_IMPORT_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_wolfram),
        ..LIST_DEFAULTS
    };
    specs[lang::SOLIDITY as usize] = CbmLangSpec {
        language: lang::SOLIDITY,
        function_node_types: SOLIDITY_FUNC_TYPES.as_ptr(),
        class_node_types: SOLIDITY_CLASS_TYPES.as_ptr(),
        field_node_types: SOLIDITY_FIELD_TYPES.as_ptr(),
        module_node_types: SOLIDITY_MODULE_TYPES.as_ptr(),
        call_node_types: SOLIDITY_CALL_TYPES.as_ptr(),
        import_node_types: SOLIDITY_IMPORT_TYPES.as_ptr(),
        branching_node_types: SOLIDITY_BRANCH_TYPES.as_ptr(),
        variable_node_types: SOLIDITY_VAR_TYPES.as_ptr(),
        assignment_node_types: SOLIDITY_ASSIGN_TYPES.as_ptr(),
        throw_node_types: SOLIDITY_THROW_TYPES.as_ptr(),
        decorator_node_types: SOLIDITY_DECORATOR_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_solidity),
        ..LIST_DEFAULTS
    };
    specs[lang::TYPST as usize] = CbmLangSpec {
        language: lang::TYPST,
        function_node_types: TYPST_FUNC_TYPES.as_ptr(),
        module_node_types: TYPST_MODULE_TYPES.as_ptr(),
        call_node_types: TYPST_CALL_TYPES.as_ptr(),
        import_node_types: TYPST_IMPORT_TYPES.as_ptr(),
        branching_node_types: TYPST_BRANCH_TYPES.as_ptr(),
        variable_node_types: TYPST_VAR_TYPES.as_ptr(),
        assignment_node_types: TYPST_ASSIGN_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_typst),
        ..LIST_DEFAULTS
    };
    specs[lang::GDSCRIPT as usize] = CbmLangSpec {
        language: lang::GDSCRIPT,
        function_node_types: GDSCRIPT_FUNC_TYPES.as_ptr(),
        class_node_types: GDSCRIPT_CLASS_TYPES.as_ptr(),
        field_node_types: GDSCRIPT_FIELD_TYPES.as_ptr(),
        module_node_types: GDSCRIPT_MODULE_TYPES.as_ptr(),
        call_node_types: GDSCRIPT_CALL_TYPES.as_ptr(),
        import_node_types: GDSCRIPT_IMPORT_TYPES.as_ptr(),
        branching_node_types: GDSCRIPT_BRANCH_TYPES.as_ptr(),
        variable_node_types: GDSCRIPT_VAR_TYPES.as_ptr(),
        assignment_node_types: GDSCRIPT_ASSIGN_TYPES.as_ptr(),
        decorator_node_types: GDSCRIPT_DECORATOR_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_gdscript),
        ..LIST_DEFAULTS
    };
    specs[lang::QML as usize] = CbmLangSpec {
        language: lang::QML,
        function_node_types: TS_FUNC_TYPES.as_ptr(),
        class_node_types: QML_CLASS_TYPES.as_ptr(),
        field_node_types: QML_FIELD_TYPES.as_ptr(),
        module_node_types: JS_MODULE_TYPES.as_ptr(),
        call_node_types: JS_CALL_TYPES.as_ptr(),
        import_node_types: QML_IMPORT_TYPES.as_ptr(),
        import_from_types: QML_IMPORT_TYPES.as_ptr(),
        branching_node_types: JS_BRANCH_TYPES.as_ptr(),
        variable_node_types: JS_VAR_TYPES.as_ptr(),
        assignment_node_types: JS_ASSIGN_TYPES.as_ptr(),
        throw_node_types: JS_THROW_TYPES.as_ptr(),
        decorator_node_types: TS_DECORATOR_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_qmljs),
        ..LIST_DEFAULTS
    };
    specs[lang::CFSCRIPT as usize] = CbmLangSpec {
        language: lang::CFSCRIPT,
        function_node_types: CFSCRIPT_FUNC_TYPES.as_ptr(),
        field_node_types: CFSCRIPT_FIELD_TYPES.as_ptr(),
        module_node_types: JS_MODULE_TYPES.as_ptr(),
        call_node_types: JS_CALL_TYPES.as_ptr(),
        import_node_types: CFSCRIPT_IMPORT_TYPES.as_ptr(),
        import_from_types: CFSCRIPT_IMPORT_TYPES.as_ptr(),
        branching_node_types: JS_BRANCH_TYPES.as_ptr(),
        variable_node_types: JS_VAR_TYPES.as_ptr(),
        assignment_node_types: JS_ASSIGN_TYPES.as_ptr(),
        throw_node_types: JS_THROW_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_cfscript),
        ..LIST_DEFAULTS
    };
    specs[lang::CFML as usize] = CbmLangSpec {
        language: lang::CFML,
        function_node_types: CFML_FUNC_TYPES.as_ptr(),
        module_node_types: CFML_MODULE_TYPES.as_ptr(),
        call_node_types: CFML_CALL_TYPES.as_ptr(),
        branching_node_types: CFML_BRANCH_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_cfml),
        embedded_imports: CFML_EMBEDDED_IMPORTS.as_ptr(),
        ..LIST_DEFAULTS
    };
    specs[lang::GLEAM as usize] = CbmLangSpec {
        language: lang::GLEAM,
        function_node_types: GLEAM_FUNC_TYPES.as_ptr(),
        class_node_types: GLEAM_CLASS_TYPES.as_ptr(),
        field_node_types: GLEAM_FIELD_TYPES.as_ptr(),
        module_node_types: GLEAM_MODULE_TYPES.as_ptr(),
        call_node_types: GLEAM_CALL_TYPES.as_ptr(),
        import_node_types: GLEAM_IMPORT_TYPES.as_ptr(),
        branching_node_types: GLEAM_BRANCH_TYPES.as_ptr(),
        variable_node_types: GLEAM_VAR_TYPES.as_ptr(),
        assignment_node_types: GLEAM_ASSIGN_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_gleam),
        ..LIST_DEFAULTS
    };
    specs[lang::POWERSHELL as usize] = CbmLangSpec {
        language: lang::POWERSHELL,
        function_node_types: POWERSHELL_FUNC_TYPES.as_ptr(),
        class_node_types: POWERSHELL_CLASS_TYPES.as_ptr(),
        module_node_types: POWERSHELL_MODULE_TYPES.as_ptr(),
        call_node_types: POWERSHELL_CALL_TYPES.as_ptr(),
        import_node_types: POWERSHELL_IMPORT_TYPES.as_ptr(),
        branching_node_types: POWERSHELL_BRANCH_TYPES.as_ptr(),
        variable_node_types: POWERSHELL_VAR_TYPES.as_ptr(),
        assignment_node_types: POWERSHELL_ASSIGN_TYPES.as_ptr(),
        throw_node_types: POWERSHELL_THROW_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_powershell),
        ..LIST_DEFAULTS
    };
    specs[lang::PASCAL as usize] = CbmLangSpec {
        language: lang::PASCAL,
        function_node_types: PASCAL_FUNC_TYPES.as_ptr(),
        class_node_types: PASCAL_CLASS_TYPES.as_ptr(),
        field_node_types: PASCAL_FIELD_TYPES.as_ptr(),
        module_node_types: PASCAL_MODULE_TYPES.as_ptr(),
        call_node_types: PASCAL_CALL_TYPES.as_ptr(),
        import_node_types: PASCAL_IMPORT_TYPES.as_ptr(),
        branching_node_types: PASCAL_BRANCH_TYPES.as_ptr(),
        variable_node_types: PASCAL_VAR_TYPES.as_ptr(),
        assignment_node_types: PASCAL_ASSIGN_TYPES.as_ptr(),
        throw_node_types: PASCAL_THROW_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_pascal),
        ..LIST_DEFAULTS
    };
    specs[lang::DLANG as usize] = CbmLangSpec {
        language: lang::DLANG,
        function_node_types: D_FUNC_TYPES.as_ptr(),
        class_node_types: D_CLASS_TYPES.as_ptr(),
        field_node_types: D_FIELD_TYPES.as_ptr(),
        module_node_types: D_MODULE_TYPES.as_ptr(),
        call_node_types: D_CALL_TYPES.as_ptr(),
        import_node_types: D_IMPORT_TYPES.as_ptr(),
        branching_node_types: D_BRANCH_TYPES.as_ptr(),
        variable_node_types: D_VAR_TYPES.as_ptr(),
        assignment_node_types: D_ASSIGN_TYPES.as_ptr(),
        throw_node_types: D_THROW_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_d),
        ..LIST_DEFAULTS
    };
    specs[lang::SCHEME as usize] = CbmLangSpec {
        language: lang::SCHEME,
        function_node_types: SCHEME_FUNC_TYPES.as_ptr(),
        module_node_types: SCHEME_MODULE_TYPES.as_ptr(),
        call_node_types: SCHEME_CALL_TYPES.as_ptr(),
        variable_node_types: SCHEME_VAR_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_scheme),
        ..LIST_DEFAULTS
    };

    // CBM_LANG_CHIALISP — lisp-family shape (generic list/symbol nodes)
    specs[lang::CHIALISP as usize] = CbmLangSpec {
        language: lang::CHIALISP,
        function_node_types: CHIALISP_FUNC_TYPES.as_ptr(),
        module_node_types: CHIALISP_MODULE_TYPES.as_ptr(),
        call_node_types: CHIALISP_CALL_TYPES.as_ptr(),
        variable_node_types: CHIALISP_VAR_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_chialisp),
        ..LIST_DEFAULTS
    };
    specs[lang::FENNEL as usize] = CbmLangSpec {
        language: lang::FENNEL,
        function_node_types: FENNEL_FUNC_TYPES.as_ptr(),
        module_node_types: FENNEL_MODULE_TYPES.as_ptr(),
        call_node_types: FENNEL_CALL_TYPES.as_ptr(),
        branching_node_types: FENNEL_BRANCH_TYPES.as_ptr(),
        variable_node_types: FENNEL_VAR_TYPES.as_ptr(),
        assignment_node_types: FENNEL_ASSIGN_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_fennel),
        ..LIST_DEFAULTS
    };
    specs[lang::FISH as usize] = CbmLangSpec {
        language: lang::FISH,
        function_node_types: FISH_FUNC_TYPES.as_ptr(),
        module_node_types: FISH_MODULE_TYPES.as_ptr(),
        call_node_types: FISH_CALL_TYPES.as_ptr(),
        branching_node_types: FISH_BRANCH_TYPES.as_ptr(),
        variable_node_types: FISH_VAR_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_fish),
        ..LIST_DEFAULTS
    };
    specs[lang::AWK as usize] = CbmLangSpec {
        language: lang::AWK,
        function_node_types: AWK_FUNC_TYPES.as_ptr(),
        module_node_types: AWK_MODULE_TYPES.as_ptr(),
        call_node_types: AWK_CALL_TYPES.as_ptr(),
        branching_node_types: AWK_BRANCH_TYPES.as_ptr(),
        variable_node_types: AWK_VAR_TYPES.as_ptr(),
        assignment_node_types: AWK_ASSIGN_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_awk),
        ..LIST_DEFAULTS
    };
    specs[lang::ZSH as usize] = CbmLangSpec {
        language: lang::ZSH,
        function_node_types: ZSH_FUNC_TYPES.as_ptr(),
        module_node_types: ZSH_MODULE_TYPES.as_ptr(),
        call_node_types: ZSH_CALL_TYPES.as_ptr(),
        branching_node_types: ZSH_BRANCH_TYPES.as_ptr(),
        variable_node_types: ZSH_VAR_TYPES.as_ptr(),
        assignment_node_types: ZSH_ASSIGN_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_zsh),
        ..LIST_DEFAULTS
    };
    specs[lang::TCL as usize] = CbmLangSpec {
        language: lang::TCL,
        function_node_types: TCL_FUNC_TYPES.as_ptr(),
        class_node_types: TCL_CLASS_TYPES.as_ptr(),
        module_node_types: TCL_MODULE_TYPES.as_ptr(),
        call_node_types: TCL_CALL_TYPES.as_ptr(),
        branching_node_types: TCL_BRANCH_TYPES.as_ptr(),
        variable_node_types: TCL_VAR_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_tcl),
        ..LIST_DEFAULTS
    };
    specs[lang::ADA as usize] = CbmLangSpec {
        language: lang::ADA,
        function_node_types: ADA_FUNC_TYPES.as_ptr(),
        class_node_types: ADA_CLASS_TYPES.as_ptr(),
        field_node_types: ADA_FIELD_TYPES.as_ptr(),
        module_node_types: ADA_MODULE_TYPES.as_ptr(),
        call_node_types: ADA_CALL_TYPES.as_ptr(),
        import_node_types: ADA_IMPORT_TYPES.as_ptr(),
        branching_node_types: ADA_BRANCH_TYPES.as_ptr(),
        variable_node_types: ADA_VAR_TYPES.as_ptr(),
        assignment_node_types: ADA_ASSIGN_TYPES.as_ptr(),
        throw_node_types: ADA_THROW_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_ada),
        ..LIST_DEFAULTS
    };
    specs[lang::AGDA as usize] = CbmLangSpec {
        language: lang::AGDA,
        function_node_types: AGDA_FUNC_TYPES.as_ptr(),
        class_node_types: AGDA_CLASS_TYPES.as_ptr(),
        module_node_types: AGDA_MODULE_TYPES.as_ptr(),
        call_node_types: AGDA_CALL_TYPES.as_ptr(),
        import_node_types: AGDA_IMPORT_TYPES.as_ptr(),
        branching_node_types: AGDA_BRANCH_TYPES.as_ptr(),
        variable_node_types: AGDA_VAR_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_agda),
        ..LIST_DEFAULTS
    };
    specs[lang::RACKET as usize] = CbmLangSpec {
        language: lang::RACKET,
        function_node_types: RACKET_FUNC_TYPES.as_ptr(),
        class_node_types: RACKET_CLASS_TYPES.as_ptr(),
        module_node_types: RACKET_MODULE_TYPES.as_ptr(),
        call_node_types: RACKET_CALL_TYPES.as_ptr(),
        variable_node_types: RACKET_VAR_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_racket),
        ..LIST_DEFAULTS
    };
    specs[lang::ODIN as usize] = CbmLangSpec {
        language: lang::ODIN,
        function_node_types: ODIN_FUNC_TYPES.as_ptr(),
        class_node_types: ODIN_CLASS_TYPES.as_ptr(),
        field_node_types: ODIN_FIELD_TYPES.as_ptr(),
        module_node_types: ODIN_MODULE_TYPES.as_ptr(),
        call_node_types: ODIN_CALL_TYPES.as_ptr(),
        import_node_types: ODIN_IMPORT_TYPES.as_ptr(),
        branching_node_types: ODIN_BRANCH_TYPES.as_ptr(),
        variable_node_types: ODIN_VAR_TYPES.as_ptr(),
        assignment_node_types: ODIN_ASSIGN_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_odin),
        ..LIST_DEFAULTS
    };
    specs[lang::RESCRIPT as usize] = CbmLangSpec {
        language: lang::RESCRIPT,
        function_node_types: RESCRIPT_FUNC_TYPES.as_ptr(),
        class_node_types: RESCRIPT_CLASS_TYPES.as_ptr(),
        module_node_types: RESCRIPT_MODULE_TYPES.as_ptr(),
        call_node_types: RESCRIPT_CALL_TYPES.as_ptr(),
        import_node_types: RESCRIPT_IMPORT_TYPES.as_ptr(),
        branching_node_types: RESCRIPT_BRANCH_TYPES.as_ptr(),
        variable_node_types: RESCRIPT_VAR_TYPES.as_ptr(),
        assignment_node_types: RESCRIPT_ASSIGN_TYPES.as_ptr(),
        throw_node_types: RESCRIPT_THROW_TYPES.as_ptr(),
        decorator_node_types: RESCRIPT_DECORATOR_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_rescript),
        ..LIST_DEFAULTS
    };
    specs[lang::PURESCRIPT as usize] = CbmLangSpec {
        language: lang::PURESCRIPT,
        function_node_types: PURESCRIPT_FUNC_TYPES.as_ptr(),
        class_node_types: PURESCRIPT_CLASS_TYPES.as_ptr(),
        module_node_types: PURESCRIPT_MODULE_TYPES.as_ptr(),
        call_node_types: PURESCRIPT_CALL_TYPES.as_ptr(),
        import_node_types: PURESCRIPT_IMPORT_TYPES.as_ptr(),
        branching_node_types: PURESCRIPT_BRANCH_TYPES.as_ptr(),
        variable_node_types: PURESCRIPT_VAR_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_purescript),
        ..LIST_DEFAULTS
    };
    specs[lang::NICKEL as usize] = CbmLangSpec {
        language: lang::NICKEL,
        function_node_types: NICKEL_FUNC_TYPES.as_ptr(),
        module_node_types: NICKEL_MODULE_TYPES.as_ptr(),
        call_node_types: NICKEL_CALL_TYPES.as_ptr(),
        import_node_types: NICKEL_IMPORT_TYPES.as_ptr(),
        branching_node_types: NICKEL_BRANCH_TYPES.as_ptr(),
        variable_node_types: NICKEL_VAR_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_nickel),
        ..LIST_DEFAULTS
    };
    specs[lang::CRYSTAL as usize] = CbmLangSpec {
        language: lang::CRYSTAL,
        function_node_types: CRYSTAL_FUNC_TYPES.as_ptr(),
        class_node_types: CRYSTAL_CLASS_TYPES.as_ptr(),
        field_node_types: CRYSTAL_FIELD_TYPES.as_ptr(),
        module_node_types: CRYSTAL_MODULE_TYPES.as_ptr(),
        call_node_types: CRYSTAL_CALL_TYPES.as_ptr(),
        import_node_types: CRYSTAL_IMPORT_TYPES.as_ptr(),
        branching_node_types: CRYSTAL_BRANCH_TYPES.as_ptr(),
        variable_node_types: CRYSTAL_VAR_TYPES.as_ptr(),
        assignment_node_types: CRYSTAL_ASSIGN_TYPES.as_ptr(),
        decorator_node_types: CRYSTAL_DECORATOR_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_crystal),
        ..LIST_DEFAULTS
    };
    specs[lang::TEAL as usize] = CbmLangSpec {
        language: lang::TEAL,
        function_node_types: TEAL_FUNC_TYPES.as_ptr(),
        class_node_types: TEAL_CLASS_TYPES.as_ptr(),
        module_node_types: TEAL_MODULE_TYPES.as_ptr(),
        call_node_types: TEAL_CALL_TYPES.as_ptr(),
        branching_node_types: TEAL_BRANCH_TYPES.as_ptr(),
        variable_node_types: TEAL_VAR_TYPES.as_ptr(),
        assignment_node_types: TEAL_ASSIGN_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_teal),
        ..LIST_DEFAULTS
    };
    specs[lang::HARE as usize] = CbmLangSpec {
        language: lang::HARE,
        function_node_types: HARE_FUNC_TYPES.as_ptr(),
        class_node_types: HARE_CLASS_TYPES.as_ptr(),
        module_node_types: HARE_MODULE_TYPES.as_ptr(),
        call_node_types: HARE_CALL_TYPES.as_ptr(),
        import_node_types: HARE_IMPORT_TYPES.as_ptr(),
        branching_node_types: HARE_BRANCH_TYPES.as_ptr(),
        variable_node_types: HARE_VAR_TYPES.as_ptr(),
        assignment_node_types: HARE_ASSIGN_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_hare),
        ..LIST_DEFAULTS
    };
    specs[lang::PONY as usize] = CbmLangSpec {
        language: lang::PONY,
        function_node_types: PONY_FUNC_TYPES.as_ptr(),
        class_node_types: PONY_CLASS_TYPES.as_ptr(),
        module_node_types: PONY_MODULE_TYPES.as_ptr(),
        call_node_types: PONY_CALL_TYPES.as_ptr(),
        import_node_types: PONY_IMPORT_TYPES.as_ptr(),
        branching_node_types: PONY_BRANCH_TYPES.as_ptr(),
        variable_node_types: PONY_VAR_TYPES.as_ptr(),
        assignment_node_types: PONY_ASSIGN_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_pony),
        ..LIST_DEFAULTS
    };
    specs[lang::LUAU as usize] = CbmLangSpec {
        language: lang::LUAU,
        function_node_types: LUAU_FUNC_TYPES.as_ptr(),
        class_node_types: LUAU_CLASS_TYPES.as_ptr(),
        module_node_types: LUAU_MODULE_TYPES.as_ptr(),
        call_node_types: LUAU_CALL_TYPES.as_ptr(),
        branching_node_types: LUAU_BRANCH_TYPES.as_ptr(),
        variable_node_types: LUAU_VAR_TYPES.as_ptr(),
        assignment_node_types: LUAU_ASSIGN_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_luau),
        ..LIST_DEFAULTS
    };
    specs[lang::JANET as usize] = CbmLangSpec {
        language: lang::JANET,
        module_node_types: JANET_MODULE_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_janet_simple),
        ..LIST_DEFAULTS
    };
    specs[lang::SWAY as usize] = CbmLangSpec {
        language: lang::SWAY,
        function_node_types: SWAY_FUNC_TYPES.as_ptr(),
        class_node_types: SWAY_CLASS_TYPES.as_ptr(),
        module_node_types: SWAY_MODULE_TYPES.as_ptr(),
        call_node_types: SWAY_CALL_TYPES.as_ptr(),
        import_node_types: SWAY_IMPORT_TYPES.as_ptr(),
        branching_node_types: SWAY_BRANCH_TYPES.as_ptr(),
        variable_node_types: SWAY_VAR_TYPES.as_ptr(),
        assignment_node_types: SWAY_ASSIGN_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_sway),
        ..LIST_DEFAULTS
    };
    specs[lang::NASM as usize] = CbmLangSpec {
        language: lang::NASM,
        function_node_types: NASM_FUNC_TYPES.as_ptr(),
        class_node_types: NASM_CLASS_TYPES.as_ptr(),
        module_node_types: NASM_MODULE_TYPES.as_ptr(),
        call_node_types: NASM_CALL_TYPES.as_ptr(),
        import_node_types: NASM_IMPORT_TYPES.as_ptr(),
        variable_node_types: NASM_VAR_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_nasm),
        ..LIST_DEFAULTS
    };
    specs[lang::ASSEMBLY as usize] = CbmLangSpec {
        language: lang::ASSEMBLY,
        function_node_types: ASSEMBLY_FUNC_TYPES.as_ptr(),
        module_node_types: ASSEMBLY_MODULE_TYPES.as_ptr(),
        variable_node_types: ASSEMBLY_VAR_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_asm),
        ..LIST_DEFAULTS
    };
    specs[lang::ASTRO as usize] = CbmLangSpec {
        language: lang::ASTRO,
        module_node_types: ASTRO_MODULE_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_astro),
        embedded_imports: ASTRO_EMBEDDED_IMPORTS.as_ptr(),
        ..LIST_DEFAULTS
    };
    specs[lang::BLADE as usize] = CbmLangSpec {
        language: lang::BLADE,
        module_node_types: BLADE_MODULE_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_blade),
        ..LIST_DEFAULTS
    };
    specs[lang::JUST as usize] = CbmLangSpec {
        language: lang::JUST,
        function_node_types: JUST_FUNC_TYPES.as_ptr(),
        module_node_types: JUST_MODULE_TYPES.as_ptr(),
        call_node_types: JUST_CALL_TYPES.as_ptr(),
        import_node_types: JUST_IMPORT_TYPES.as_ptr(),
        branching_node_types: JUST_BRANCH_TYPES.as_ptr(),
        assignment_node_types: JUST_ASSIGN_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_just),
        ..LIST_DEFAULTS
    };
    specs[lang::GOTEMPLATE as usize] = CbmLangSpec {
        language: lang::GOTEMPLATE,
        function_node_types: GOTEMPLATE_FUNC_TYPES.as_ptr(),
        module_node_types: GOTEMPLATE_MODULE_TYPES.as_ptr(),
        call_node_types: GOTEMPLATE_CALL_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_gotmpl),
        ..LIST_DEFAULTS
    };
    specs[lang::TEMPL as usize] = CbmLangSpec {
        language: lang::TEMPL,
        function_node_types: TEMPL_FUNC_TYPES.as_ptr(),
        class_node_types: TEMPL_CLASS_TYPES.as_ptr(),
        module_node_types: TEMPL_MODULE_TYPES.as_ptr(),
        call_node_types: TEMPL_CALL_TYPES.as_ptr(),
        import_node_types: TEMPL_IMPORT_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_templ),
        ..LIST_DEFAULTS
    };
    specs[lang::LIQUID as usize] = CbmLangSpec {
        language: lang::LIQUID,
        module_node_types: LIQUID_MODULE_TYPES.as_ptr(),
        import_node_types: LIQUID_IMPORT_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_liquid),
        ..LIST_DEFAULTS
    };
    specs[lang::JINJA2 as usize] = CbmLangSpec {
        language: lang::JINJA2,
        module_node_types: JINJA2_MODULE_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_jinja2),
        ..LIST_DEFAULTS
    };
    specs[lang::PRISMA as usize] = CbmLangSpec {
        language: lang::PRISMA,
        class_node_types: PRISMA_CLASS_TYPES.as_ptr(),
        field_node_types: PRISMA_FIELD_TYPES.as_ptr(),
        module_node_types: PRISMA_MODULE_TYPES.as_ptr(),
        call_node_types: PRISMA_CALL_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_prisma),
        ..LIST_DEFAULTS
    };
    specs[lang::HYPRLANG as usize] = CbmLangSpec {
        language: lang::HYPRLANG,
        module_node_types: HYPRLANG_MODULE_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_hyprlang),
        ..LIST_DEFAULTS
    };
    specs[lang::DOTENV as usize] = CbmLangSpec {
        language: lang::DOTENV,
        module_node_types: DOTENV_MODULE_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_dotenv),
        ..LIST_DEFAULTS
    };
    specs[lang::DIFF as usize] = CbmLangSpec {
        language: lang::DIFF,
        module_node_types: DIFF_MODULE_TYPES.as_ptr(),
        call_node_types: DIFF_CALL_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_diff),
        ..LIST_DEFAULTS
    };
    specs[lang::WGSL as usize] = CbmLangSpec {
        language: lang::WGSL,
        function_node_types: WGSL_FUNC_TYPES.as_ptr(),
        class_node_types: WGSL_CLASS_TYPES.as_ptr(),
        module_node_types: WGSL_MODULE_TYPES.as_ptr(),
        call_node_types: WGSL_CALL_TYPES.as_ptr(),
        import_node_types: WGSL_IMPORT_TYPES.as_ptr(),
        branching_node_types: WGSL_BRANCH_TYPES.as_ptr(),
        variable_node_types: WGSL_VAR_TYPES.as_ptr(),
        assignment_node_types: WGSL_ASSIGN_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_wgsl),
        ..LIST_DEFAULTS
    };
    specs[lang::KDL as usize] = CbmLangSpec {
        language: lang::KDL,
        module_node_types: KDL_MODULE_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_kdl),
        ..LIST_DEFAULTS
    };
    specs[lang::JSON5 as usize] = CbmLangSpec {
        language: lang::JSON5,
        module_node_types: JSON5_MODULE_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_json5),
        ..LIST_DEFAULTS
    };
    specs[lang::JSONNET as usize] = CbmLangSpec {
        language: lang::JSONNET,
        function_node_types: JSONNET_FUNC_TYPES.as_ptr(),
        module_node_types: JSONNET_MODULE_TYPES.as_ptr(),
        call_node_types: JSONNET_CALL_TYPES.as_ptr(),
        import_node_types: JSONNET_IMPORT_TYPES.as_ptr(),
        branching_node_types: JSONNET_BRANCH_TYPES.as_ptr(),
        variable_node_types: JSONNET_VAR_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_jsonnet),
        ..LIST_DEFAULTS
    };
    specs[lang::RON as usize] = CbmLangSpec {
        language: lang::RON,
        module_node_types: RON_MODULE_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_ron),
        ..LIST_DEFAULTS
    };
    specs[lang::THRIFT as usize] = CbmLangSpec {
        language: lang::THRIFT,
        function_node_types: THRIFT_FUNC_TYPES.as_ptr(),
        class_node_types: THRIFT_CLASS_TYPES.as_ptr(),
        field_node_types: THRIFT_FIELD_TYPES.as_ptr(),
        module_node_types: THRIFT_MODULE_TYPES.as_ptr(),
        import_node_types: THRIFT_IMPORT_TYPES.as_ptr(),
        variable_node_types: THRIFT_VAR_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_thrift),
        ..LIST_DEFAULTS
    };
    specs[lang::CAPNP as usize] = CbmLangSpec {
        language: lang::CAPNP,
        function_node_types: CAPNP_FUNC_TYPES.as_ptr(),
        class_node_types: CAPNP_CLASS_TYPES.as_ptr(),
        field_node_types: CAPNP_FIELD_TYPES.as_ptr(),
        module_node_types: CAPNP_MODULE_TYPES.as_ptr(),
        import_node_types: CAPNP_IMPORT_TYPES.as_ptr(),
        variable_node_types: CAPNP_VAR_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_capnp),
        ..LIST_DEFAULTS
    };
    specs[lang::PROPERTIES as usize] = CbmLangSpec {
        language: lang::PROPERTIES,
        module_node_types: PROPERTIES_MODULE_TYPES.as_ptr(),
        variable_node_types: PROPERTIES_VAR_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_properties),
        ..LIST_DEFAULTS
    };
    specs[lang::SSHCONFIG as usize] = CbmLangSpec {
        language: lang::SSHCONFIG,
        module_node_types: SSHCONFIG_MODULE_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_ssh_config),
        ..LIST_DEFAULTS
    };
    specs[lang::BIBTEX as usize] = CbmLangSpec {
        language: lang::BIBTEX,
        module_node_types: BIBTEX_MODULE_TYPES.as_ptr(),
        call_node_types: BIBTEX_CALL_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_bibtex),
        ..LIST_DEFAULTS
    };
    specs[lang::STARLARK as usize] = CbmLangSpec {
        language: lang::STARLARK,
        function_node_types: STARLARK_FUNC_TYPES.as_ptr(),
        module_node_types: STARLARK_MODULE_TYPES.as_ptr(),
        call_node_types: STARLARK_CALL_TYPES.as_ptr(),
        import_node_types: STARLARK_IMPORT_TYPES.as_ptr(),
        branching_node_types: STARLARK_BRANCH_TYPES.as_ptr(),
        variable_node_types: STARLARK_VAR_TYPES.as_ptr(),
        assignment_node_types: STARLARK_ASSIGN_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_starlark),
        ..LIST_DEFAULTS
    };
    specs[lang::BICEP as usize] = CbmLangSpec {
        language: lang::BICEP,
        function_node_types: BICEP_FUNC_TYPES.as_ptr(),
        class_node_types: BICEP_CLASS_TYPES.as_ptr(),
        module_node_types: BICEP_MODULE_TYPES.as_ptr(),
        call_node_types: BICEP_CALL_TYPES.as_ptr(),
        import_node_types: BICEP_IMPORT_TYPES.as_ptr(),
        variable_node_types: BICEP_VAR_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_bicep),
        ..LIST_DEFAULTS
    };
    specs[lang::CSV as usize] = CbmLangSpec {
        language: lang::CSV,
        module_node_types: CSV_MODULE_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_csv),
        ..LIST_DEFAULTS
    };
    specs[lang::REQUIREMENTS as usize] = CbmLangSpec {
        language: lang::REQUIREMENTS,
        module_node_types: REQUIREMENTS_MODULE_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_requirements),
        ..LIST_DEFAULTS
    };
    specs[lang::HLSL as usize] = CbmLangSpec {
        language: lang::HLSL,
        function_node_types: HLSL_FUNC_TYPES.as_ptr(),
        class_node_types: HLSL_CLASS_TYPES.as_ptr(),
        module_node_types: HLSL_MODULE_TYPES.as_ptr(),
        call_node_types: HLSL_CALL_TYPES.as_ptr(),
        import_node_types: HLSL_IMPORT_TYPES.as_ptr(),
        branching_node_types: HLSL_BRANCH_TYPES.as_ptr(),
        variable_node_types: HLSL_VAR_TYPES.as_ptr(),
        assignment_node_types: HLSL_ASSIGN_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_hlsl),
        ..LIST_DEFAULTS
    };
    specs[lang::VHDL as usize] = CbmLangSpec {
        language: lang::VHDL,
        function_node_types: VHDL_FUNC_TYPES.as_ptr(),
        class_node_types: VHDL_CLASS_TYPES.as_ptr(),
        module_node_types: VHDL_MODULE_TYPES.as_ptr(),
        call_node_types: VHDL_CALL_TYPES.as_ptr(),
        import_node_types: VHDL_IMPORT_TYPES.as_ptr(),
        branching_node_types: VHDL_BRANCH_TYPES.as_ptr(),
        variable_node_types: VHDL_VAR_TYPES.as_ptr(),
        assignment_node_types: VHDL_ASSIGN_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_vhdl),
        ..LIST_DEFAULTS
    };
    specs[lang::SYSTEMVERILOG as usize] = CbmLangSpec {
        language: lang::SYSTEMVERILOG,
        function_node_types: SYSTEMVERILOG_FUNC_TYPES.as_ptr(),
        class_node_types: SYSTEMVERILOG_CLASS_TYPES.as_ptr(),
        module_node_types: SYSTEMVERILOG_MODULE_TYPES.as_ptr(),
        call_node_types: SYSTEMVERILOG_CALL_TYPES.as_ptr(),
        import_node_types: SYSTEMVERILOG_IMPORT_TYPES.as_ptr(),
        branching_node_types: SYSTEMVERILOG_BRANCH_TYPES.as_ptr(),
        variable_node_types: SYSTEMVERILOG_VAR_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_systemverilog),
        ..LIST_DEFAULTS
    };
    specs[lang::DEVICETREE as usize] = CbmLangSpec {
        language: lang::DEVICETREE,
        module_node_types: DEVICETREE_MODULE_TYPES.as_ptr(),
        call_node_types: DEVICETREE_CALL_TYPES.as_ptr(),
        import_node_types: DEVICETREE_IMPORT_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_devicetree),
        ..LIST_DEFAULTS
    };
    specs[lang::LINKERSCRIPT as usize] = CbmLangSpec {
        language: lang::LINKERSCRIPT,
        module_node_types: LINKERSCRIPT_MODULE_TYPES.as_ptr(),
        call_node_types: LINKERSCRIPT_CALL_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_linkerscript),
        ..LIST_DEFAULTS
    };
    specs[lang::GN as usize] = CbmLangSpec {
        language: lang::GN,
        module_node_types: GN_MODULE_TYPES.as_ptr(),
        call_node_types: GN_CALL_TYPES.as_ptr(),
        import_node_types: GN_IMPORT_TYPES.as_ptr(),
        branching_node_types: GN_BRANCH_TYPES.as_ptr(),
        assignment_node_types: GN_ASSIGN_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_gn),
        ..LIST_DEFAULTS
    };
    specs[lang::KCONFIG as usize] = CbmLangSpec {
        language: lang::KCONFIG,
        class_node_types: KCONFIG_CLASS_TYPES.as_ptr(),
        module_node_types: KCONFIG_MODULE_TYPES.as_ptr(),
        import_node_types: KCONFIG_IMPORT_TYPES.as_ptr(),
        branching_node_types: KCONFIG_BRANCH_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_kconfig),
        ..LIST_DEFAULTS
    };
    specs[lang::BITBAKE as usize] = CbmLangSpec {
        language: lang::BITBAKE,
        function_node_types: BITBAKE_FUNC_TYPES.as_ptr(),
        module_node_types: BITBAKE_MODULE_TYPES.as_ptr(),
        call_node_types: BITBAKE_CALL_TYPES.as_ptr(),
        import_node_types: BITBAKE_IMPORT_TYPES.as_ptr(),
        variable_node_types: BITBAKE_VAR_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_bitbake),
        ..LIST_DEFAULTS
    };
    specs[lang::SMALI as usize] = CbmLangSpec {
        language: lang::SMALI,
        function_node_types: SMALI_FUNC_TYPES.as_ptr(),
        class_node_types: SMALI_CLASS_TYPES.as_ptr(),
        field_node_types: SMALI_FIELD_TYPES.as_ptr(),
        module_node_types: SMALI_MODULE_TYPES.as_ptr(),
        import_node_types: SMALI_IMPORT_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_smali),
        ..LIST_DEFAULTS
    };
    specs[lang::TABLEGEN as usize] = CbmLangSpec {
        language: lang::TABLEGEN,
        function_node_types: TABLEGEN_FUNC_TYPES.as_ptr(),
        class_node_types: TABLEGEN_CLASS_TYPES.as_ptr(),
        module_node_types: TABLEGEN_MODULE_TYPES.as_ptr(),
        import_node_types: TABLEGEN_IMPORT_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_tablegen),
        ..LIST_DEFAULTS
    };
    specs[lang::ISPC as usize] = CbmLangSpec {
        language: lang::ISPC,
        function_node_types: ISPC_FUNC_TYPES.as_ptr(),
        class_node_types: ISPC_CLASS_TYPES.as_ptr(),
        module_node_types: ISPC_MODULE_TYPES.as_ptr(),
        call_node_types: ISPC_CALL_TYPES.as_ptr(),
        import_node_types: ISPC_IMPORT_TYPES.as_ptr(),
        branching_node_types: ISPC_BRANCH_TYPES.as_ptr(),
        variable_node_types: ISPC_VAR_TYPES.as_ptr(),
        assignment_node_types: ISPC_ASSIGN_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_ispc),
        ..LIST_DEFAULTS
    };
    specs[lang::CAIRO as usize] = CbmLangSpec {
        language: lang::CAIRO,
        function_node_types: CAIRO_FUNC_TYPES.as_ptr(),
        class_node_types: CAIRO_CLASS_TYPES.as_ptr(),
        module_node_types: CAIRO_MODULE_TYPES.as_ptr(),
        call_node_types: CAIRO_CALL_TYPES.as_ptr(),
        import_node_types: CAIRO_IMPORT_TYPES.as_ptr(),
        branching_node_types: CAIRO_BRANCH_TYPES.as_ptr(),
        variable_node_types: CAIRO_VAR_TYPES.as_ptr(),
        assignment_node_types: CAIRO_ASSIGN_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_cairo),
        ..LIST_DEFAULTS
    };
    specs[lang::MOVE as usize] = CbmLangSpec {
        language: lang::MOVE,
        function_node_types: MOVE_FUNC_TYPES.as_ptr(),
        module_node_types: MOVE_MODULE_TYPES.as_ptr(),
        call_node_types: MOVE_CALL_TYPES.as_ptr(),
        import_node_types: MOVE_IMPORT_TYPES.as_ptr(),
        branching_node_types: MOVE_BRANCH_TYPES.as_ptr(),
        variable_node_types: MOVE_VAR_TYPES.as_ptr(),
        assignment_node_types: MOVE_ASSIGN_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_move),
        ..LIST_DEFAULTS
    };
    specs[lang::SQUIRREL as usize] = CbmLangSpec {
        language: lang::SQUIRREL,
        function_node_types: SQUIRREL_FUNC_TYPES.as_ptr(),
        class_node_types: SQUIRREL_CLASS_TYPES.as_ptr(),
        module_node_types: SQUIRREL_MODULE_TYPES.as_ptr(),
        call_node_types: SQUIRREL_CALL_TYPES.as_ptr(),
        import_node_types: SQUIRREL_IMPORT_TYPES.as_ptr(),
        branching_node_types: SQUIRREL_BRANCH_TYPES.as_ptr(),
        variable_node_types: SQUIRREL_VAR_TYPES.as_ptr(),
        assignment_node_types: SQUIRREL_ASSIGN_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_squirrel),
        ..LIST_DEFAULTS
    };
    specs[lang::FUNC as usize] = CbmLangSpec {
        language: lang::FUNC,
        function_node_types: FUNC_FUNC_TYPES.as_ptr(),
        module_node_types: FUNC_MODULE_TYPES.as_ptr(),
        call_node_types: FUNC_CALL_TYPES.as_ptr(),
        import_node_types: FUNC_IMPORT_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_func),
        ..LIST_DEFAULTS
    };
    specs[lang::REGEX as usize] = CbmLangSpec {
        language: lang::REGEX,
        module_node_types: REGEX_MODULE_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_regex),
        ..LIST_DEFAULTS
    };
    specs[lang::JSDOC as usize] = CbmLangSpec {
        language: lang::JSDOC,
        module_node_types: JSDOC_MODULE_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_jsdoc),
        ..LIST_DEFAULTS
    };
    specs[lang::RST as usize] = CbmLangSpec {
        language: lang::RST,
        module_node_types: RST_MODULE_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_rst),
        ..LIST_DEFAULTS
    };
    specs[lang::BEANCOUNT as usize] = CbmLangSpec {
        language: lang::BEANCOUNT,
        module_node_types: BEANCOUNT_MODULE_TYPES.as_ptr(),
        import_node_types: BEANCOUNT_IMPORT_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_beancount),
        ..LIST_DEFAULTS
    };
    specs[lang::MERMAID as usize] = CbmLangSpec {
        language: lang::MERMAID,
        module_node_types: MERMAID_MODULE_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_mermaid),
        ..LIST_DEFAULTS
    };
    specs[lang::PUPPET as usize] = CbmLangSpec {
        language: lang::PUPPET,
        function_node_types: PUPPET_FUNC_TYPES.as_ptr(),
        class_node_types: PUPPET_CLASS_TYPES.as_ptr(),
        module_node_types: PUPPET_MODULE_TYPES.as_ptr(),
        call_node_types: PUPPET_CALL_TYPES.as_ptr(),
        import_node_types: PUPPET_IMPORT_TYPES.as_ptr(),
        branching_node_types: PUPPET_BRANCH_TYPES.as_ptr(),
        variable_node_types: PUPPET_VAR_TYPES.as_ptr(),
        assignment_node_types: PUPPET_ASSIGN_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_puppet),
        ..LIST_DEFAULTS
    };
    specs[lang::PO as usize] = CbmLangSpec {
        language: lang::PO,
        module_node_types: PO_MODULE_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_po),
        ..LIST_DEFAULTS
    };
    specs[lang::GITATTRIBUTES as usize] = CbmLangSpec {
        language: lang::GITATTRIBUTES,
        module_node_types: GITATTRIBUTES_MODULE_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_gitattributes),
        ..LIST_DEFAULTS
    };
    specs[lang::GITIGNORE as usize] = CbmLangSpec {
        language: lang::GITIGNORE,
        module_node_types: GITIGNORE_MODULE_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_gitignore),
        ..LIST_DEFAULTS
    };
    specs[lang::SLANG as usize] = CbmLangSpec {
        language: lang::SLANG,
        function_node_types: SLANG_FUNC_TYPES.as_ptr(),
        class_node_types: SLANG_CLASS_TYPES.as_ptr(),
        module_node_types: SLANG_MODULE_TYPES.as_ptr(),
        call_node_types: SLANG_CALL_TYPES.as_ptr(),
        import_node_types: SLANG_IMPORT_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_slang),
        ..LIST_DEFAULTS
    };
    specs[lang::LLVM_IR as usize] = CbmLangSpec {
        language: lang::LLVM_IR,
        function_node_types: LLVM_FUNC_TYPES.as_ptr(),
        module_node_types: LLVM_MODULE_TYPES.as_ptr(),
        call_node_types: LLVM_CALL_TYPES.as_ptr(),
        branching_node_types: LLVM_BRANCH_TYPES.as_ptr(),
        variable_node_types: LLVM_VAR_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_llvm),
        ..LIST_DEFAULTS
    };
    specs[lang::SMITHY as usize] = CbmLangSpec {
        language: lang::SMITHY,
        function_node_types: SMITHY_FUNC_TYPES.as_ptr(),
        class_node_types: SMITHY_CLASS_TYPES.as_ptr(),
        field_node_types: SMITHY_FIELD_TYPES.as_ptr(),
        module_node_types: SMITHY_MODULE_TYPES.as_ptr(),
        import_node_types: SMITHY_IMPORT_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_smithy),
        ..LIST_DEFAULTS
    };
    specs[lang::WIT as usize] = CbmLangSpec {
        language: lang::WIT,
        function_node_types: WIT_FUNC_TYPES.as_ptr(),
        class_node_types: WIT_CLASS_TYPES.as_ptr(),
        field_node_types: WIT_FIELD_TYPES.as_ptr(),
        module_node_types: WIT_MODULE_TYPES.as_ptr(),
        import_node_types: WIT_IMPORT_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_wit),
        ..LIST_DEFAULTS
    };
    specs[lang::TLAPLUS as usize] = CbmLangSpec {
        language: lang::TLAPLUS,
        function_node_types: TLAPLUS_FUNC_TYPES.as_ptr(),
        module_node_types: TLAPLUS_MODULE_TYPES.as_ptr(),
        call_node_types: TLAPLUS_CALL_TYPES.as_ptr(),
        import_node_types: TLAPLUS_IMPORT_TYPES.as_ptr(),
        branching_node_types: TLAPLUS_BRANCH_TYPES.as_ptr(),
        variable_node_types: TLAPLUS_VAR_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_tlaplus),
        ..LIST_DEFAULTS
    };
    specs[lang::PKL as usize] = CbmLangSpec {
        language: lang::PKL,
        function_node_types: PKL_FUNC_TYPES.as_ptr(),
        class_node_types: PKL_CLASS_TYPES.as_ptr(),
        module_node_types: PKL_MODULE_TYPES.as_ptr(),
        call_node_types: PKL_CALL_TYPES.as_ptr(),
        import_node_types: PKL_IMPORT_TYPES.as_ptr(),
        branching_node_types: PKL_BRANCH_TYPES.as_ptr(),
        variable_node_types: PKL_VAR_TYPES.as_ptr(),
        throw_node_types: PKL_THROW_TYPES.as_ptr(),
        decorator_node_types: PKL_DECORATOR_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_pkl),
        ..LIST_DEFAULTS
    };
    specs[lang::GOMOD as usize] = CbmLangSpec {
        language: lang::GOMOD,
        module_node_types: GOMOD_MODULE_TYPES.as_ptr(),
        import_node_types: GOMOD_IMPORT_TYPES.as_ptr(),
        variable_node_types: GOMOD_VAR_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_gomod),
        ..LIST_DEFAULTS
    };
    specs[lang::APEX as usize] = CbmLangSpec {
        language: lang::APEX,
        function_node_types: APEX_FUNC_TYPES.as_ptr(),
        class_node_types: APEX_CLASS_TYPES.as_ptr(),
        field_node_types: APEX_FIELD_TYPES.as_ptr(),
        module_node_types: APEX_MODULE_TYPES.as_ptr(),
        call_node_types: APEX_CALL_TYPES.as_ptr(),
        import_node_types: APEX_IMPORT_TYPES.as_ptr(),
        branching_node_types: APEX_BRANCH_TYPES.as_ptr(),
        variable_node_types: APEX_VAR_TYPES.as_ptr(),
        assignment_node_types: APEX_ASSIGN_TYPES.as_ptr(),
        throw_node_types: APEX_THROW_TYPES.as_ptr(),
        decorator_node_types: APEX_DECORATOR_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_apex),
        ..LIST_DEFAULTS
    };
    specs[lang::SOQL as usize] = CbmLangSpec {
        language: lang::SOQL,
        module_node_types: SOQL_MODULE_TYPES.as_ptr(),
        import_node_types: SOQL_IMPORT_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_soql),
        ..LIST_DEFAULTS
    };
    specs[lang::SOSL as usize] = CbmLangSpec {
        language: lang::SOSL,
        module_node_types: SOSL_MODULE_TYPES.as_ptr(),
        import_node_types: SOSL_IMPORT_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_sosl),
        ..LIST_DEFAULTS
    };

    // CBM_LANG_KUSTOMIZE — reuses YAML grammar; semantic extraction via cbm_extract_k8s()
    specs[lang::KUSTOMIZE as usize] = CbmLangSpec {
        language: lang::KUSTOMIZE,
        function_node_types: YAML_VAR_TYPES.as_ptr(),
        module_node_types: YAML_MODULE_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_yaml),
        ..LIST_DEFAULTS
    };

    // CBM_LANG_K8S — reuses YAML grammar; semantic extraction via cbm_extract_k8s()
    specs[lang::K8S as usize] = CbmLangSpec {
        language: lang::K8S,
        function_node_types: YAML_VAR_TYPES.as_ptr(),
        module_node_types: YAML_MODULE_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_yaml),
        ..LIST_DEFAULTS
    };

    // CBM_LANG_PINE — Pine Script (TradingView). kvarenzn/tree-sitter-pine (ISC).
    specs[lang::PINE as usize] = CbmLangSpec {
        language: lang::PINE,
        function_node_types: PINE_FUNC_TYPES.as_ptr(),
        class_node_types: PINE_CLASS_TYPES.as_ptr(),
        module_node_types: PINE_MODULE_TYPES.as_ptr(),
        call_node_types: PINE_CALL_TYPES.as_ptr(),
        branching_node_types: PINE_BRANCH_TYPES.as_ptr(),
        variable_node_types: PINE_VAR_TYPES.as_ptr(),
        assignment_node_types: PINE_ASSIGN_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_pine),
        ..LIST_DEFAULTS
    };
    specs[lang::MOJO as usize] = CbmLangSpec {
        language: lang::MOJO,
        function_node_types: MOJO_FUNC_TYPES.as_ptr(),
        class_node_types: MOJO_CLASS_TYPES.as_ptr(),
        module_node_types: MOJO_MODULE_TYPES.as_ptr(),
        call_node_types: MOJO_CALL_TYPES.as_ptr(),
        import_node_types: MOJO_IMPORT_TYPES.as_ptr(),
        import_from_types: MOJO_IMPORT_TYPES.as_ptr(),
        branching_node_types: MOJO_BRANCH_TYPES.as_ptr(),
        variable_node_types: MOJO_VAR_TYPES.as_ptr(),
        assignment_node_types: MOJO_ASSIGN_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_mojo),
        ..LIST_DEFAULTS
    };

    // CBM_LANG_OBJECTSCRIPT_UDL — InterSystems ObjectScript class (.cls) UDL.
    // intersystems/tree-sitter-objectscript.
    specs[lang::OBJECTSCRIPT_UDL as usize] = CbmLangSpec {
        language: lang::OBJECTSCRIPT_UDL,
        function_node_types: OBJECTSCRIPT_UDL_FUNC_TYPES.as_ptr(),
        class_node_types: OBJECTSCRIPT_UDL_CLASS_TYPES.as_ptr(),
        field_node_types: OBJECTSCRIPT_UDL_FIELD_TYPES.as_ptr(),
        module_node_types: OBJECTSCRIPT_UDL_MODULE_TYPES.as_ptr(),
        call_node_types: OBJECTSCRIPT_UDL_CALL_TYPES.as_ptr(),
        branching_node_types: OBJECTSCRIPT_UDL_BRANCH_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_objectscript_udl),
        ..LIST_DEFAULTS
    };

    // CBM_LANG_OBJECTSCRIPT_ROUTINE — InterSystems ObjectScript routine (.mac/.int/.rtn/.inc).
    specs[lang::OBJECTSCRIPT_ROUTINE as usize] = CbmLangSpec {
        language: lang::OBJECTSCRIPT_ROUTINE,
        function_node_types: OBJECTSCRIPT_ROUTINE_FUNC_TYPES.as_ptr(),
        module_node_types: OBJECTSCRIPT_ROUTINE_MODULE_TYPES.as_ptr(),
        call_node_types: OBJECTSCRIPT_ROUTINE_CALL_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_objectscript_routine),
        ..LIST_DEFAULTS
    };

    // CBM_LANG_OPENAPI — openapi.json / swagger.json: the JSON grammar without
    // JSON's node per key (thousands in a generated spec); cbm_extract_openapi
    // (openapi.c) reads the operations from the tree.
    specs[lang::OPENAPI as usize] = CbmLangSpec {
        language: lang::OPENAPI,
        module_node_types: JSON_MODULE_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_json),
        ..LIST_DEFAULTS
    };

    // CBM_LANG_PDF — PDF documents. No grammar row: extraction reads the text
    // layer itself (cbm_pdf_extract_document, doc_pdf.c) before any grammar is
    // looked up, so this language never reaches cbm_lang_spec()/cbm_ts_language().
    // Left as a zero spec.
    // CBM_LANG_ASCIIDOC — AsciiDoc documents. No grammar row, as PDF: extraction
    // reads the file itself (cbm_adoc_extract_document, doc_adoc.c). Zero spec.
    // CBM_LANG_DIAGRAM — diagram files. No grammar row, as PDF: extraction reads
    // the file itself (cbm_extract_diagram_file, diagram.c). Zero spec.
    // CBM_LANG_OBJECTSCRIPT_EXPORT — Studio Export XML. No grammar row: the
    // pipeline transcodes Export XML to UDL (iris_export_xml.c) and re-extracts
    // each class as CBM_LANG_OBJECTSCRIPT_UDL, so this language never reaches
    // cbm_lang_spec()/cbm_ts_language() directly. Left as a zero spec.
    specs[lang::ARKTS as usize] = CbmLangSpec {
        language: lang::ARKTS,
        function_node_types: TS_FUNC_TYPES.as_ptr(),
        class_node_types: ARKTS_CLASS_TYPES.as_ptr(),
        field_node_types: ARKTS_FIELD_TYPES.as_ptr(),
        module_node_types: JS_MODULE_TYPES.as_ptr(),
        call_node_types: JS_CALL_TYPES.as_ptr(),
        import_node_types: JS_IMPORT_TYPES.as_ptr(),
        import_from_types: JS_IMPORT_TYPES.as_ptr(),
        branching_node_types: JS_BRANCH_TYPES.as_ptr(),
        variable_node_types: JS_VAR_TYPES.as_ptr(),
        assignment_node_types: JS_ASSIGN_TYPES.as_ptr(),
        throw_node_types: JS_THROW_TYPES.as_ptr(),
        decorator_node_types: TS_DECORATOR_TYPES.as_ptr(),
        env_access_member_patterns: TS_ENV_MEMBERS.as_ptr(),
        ts_factory: Some(tree_sitter_arkts),
        ..LIST_DEFAULTS
    };

    // CBM_LANG_PLSQL — Oracle PL/SQL. AndreasMaierDe/tree-sitter-plsql (MIT).
    specs[lang::PLSQL as usize] = CbmLangSpec {
        language: lang::PLSQL,
        function_node_types: PLSQL_FUNC_TYPES.as_ptr(),
        class_node_types: PLSQL_CLASS_TYPES.as_ptr(),
        module_node_types: PLSQL_MODULE_TYPES.as_ptr(),
        call_node_types: PLSQL_CALL_TYPES.as_ptr(),
        branching_node_types: PLSQL_BRANCH_TYPES.as_ptr(),
        assignment_node_types: PLSQL_ASSIGN_TYPES.as_ptr(),
        throw_node_types: PLSQL_THROW_TYPES.as_ptr(),
        ts_factory: Some(tree_sitter_plsql),
        ..LIST_DEFAULTS
    };
    specs
};

// Conditional-compilation blocks per grammar (the
// `*_in_field_declaration_list` / `*_in_top_level` forms are aliased to these
// names by the grammars).
static C_FAMILY_CONDITIONAL_TYPES: Kinds<5> = kinds([
    c"preproc_if",
    c"preproc_ifdef",
    c"preproc_else",
    c"preproc_elif",
    c"preproc_elifdef",
]);

/// The node kinds that wrap conditionally compiled alternatives for `lang`,
/// or NULL when its grammar has none.
#[unsafe(no_mangle)]
pub extern "C" fn cbm_conditional_block_types(
    lang: c_int,
) -> *const *const c_char {
    match lang {
        lang::C
        | lang::CPP
        | lang::CUDA
        | lang::OBJC
        | lang::CSHARP
        | lang::GLSL
        | lang::HLSL
        | lang::ISPC
        | lang::SLANG => C_FAMILY_CONDITIONAL_TYPES.as_ptr(),
        _ => ptr::null(),
    }
}

fn lang_spec(lang: c_int) -> Option<&'static CbmLangSpec> {
    let spec = LANG_SPECS.get(usize::try_from(lang).ok()?)?;
    (spec.language == lang && !spec.function_node_types.is_null())
        .then_some(spec)
}

/// The spec row for `lang`, or NULL for an unsupported language.
#[unsafe(no_mangle)]
pub extern "C" fn cbm_lang_spec(lang: c_int) -> *const CbmLangSpec {
    lang_spec(lang).map_or(ptr::null(), ptr::from_ref)
}

/// The Tree-sitter grammar for `lang`, or NULL for an unsupported language.
#[unsafe(no_mangle)]
pub extern "C" fn cbm_ts_language(lang: c_int) -> *const RawLanguage {
    match lang_spec(lang).and_then(|spec| spec.ts_factory) {
        // SAFETY: a grammar factory takes no input and returns static data.
        Some(factory) => unsafe { factory() },
        None => ptr::null(),
    }
}
