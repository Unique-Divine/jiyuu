//! The managed JSON, JSONC, and JSON5 editor behind cli/config_json_like.h.
//! Source spans keep comments and formatting intact outside each edit.

mod abi;
mod edit;
mod file;
mod parser;

const MAX_BYTES: usize = 16 * 1024 * 1024;
const MAX_DEPTH: usize = 64;
const MAX_KEY_BYTES: usize = 4096;

#[derive(Debug)]
struct Invalid;

type Result<T> = std::result::Result<T, Invalid>;
