// use clap::{AppSettings, Command, SubCommand};
use run_bash::run;

pub fn main() -> anyhow::Result<()> {
    run::run_app()
}
