use crate::cmds;
use clap::Command;

pub fn run_app() -> anyhow::Result<()> {
    let mut cmd = Command::new("mycli")
        .about("CLI for convenient bash execution")
        // .setting (AppSettings::SubcommandRequiredElseHelp)
        .subcommand(cmds::go::clap_cmd())
        .subcommand(Command::new("rust").about("Rust related tasks").alias("rs"))
        .subcommand(Command::new("nibi").about("Nibi related tasks"));

    match cmd.clone().get_matches().subcommand_name() {
        Some("go") => cmds::go::exec(cmd)?,
        Some("rust") => rust_command(),
        Some("nibi") => nibi_command(),
        _ => cmd.print_help()?,
    }
    Ok(())
}

fn rust_command() {
    // Logic for rust command
    println!("Rust command executed");
}

fn nibi_command() {
    // Logic for nibi command
    println!("Nibi command executed");
}

#[cfg(test)]
pub mod tests {

    #[test]
    fn main_runs() -> anyhow::Result<()> {
        let mut cmd = assert_cmd::Command::cargo_bin("mycli")?;
        cmd.assert().success();
        Ok(())
    }
}
