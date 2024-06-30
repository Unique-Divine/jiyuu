//! run_sh.rs: Implements a script for piping shell outputs in other programs.
use std::env;

use run_sh::bash;

pub fn main() -> anyhow::Result<()> {
    let args: Vec<String> = env::args().collect();
    let cmd = args[1..].join(" ");
    bash::run_bash_and_print(cmd)?;
    Ok(())
}
