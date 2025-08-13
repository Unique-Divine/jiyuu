pub mod go {
    use clap::Command;

    use crate::bash;

    pub fn clap_cmd() -> clap::Command {
        clap::Command::new("go")
            .about("Golang-related commands")
            .subcommands([
                Command::new("test-short")
                    .alias("ts")
                    .about("Run go unit tests with coverage report"),
                Command::new("test-int")
                    .alias("ti")
                    .about("Run Go integration tests with coverage report"),
                Command::new("lint").about("Run golangci-lint"),
                Command::new("test").about("Run Go tests"),
            ])
    }

    pub fn exec(cmd: Command) -> anyhow::Result<()> {
        match cmd.clone().get_matches().subcommand_name() {
            Some("test-short") => {
                bash::run_bash_and_print(String::from(
                    r#"go test ./... -short -cover | grep -v "no test" | grep -v "no statement""#,
                ))?;
                Ok(())
            }
            Some("test-int") => {
                bash::run_bash_and_print(String::from(
                    r#"go test ./... -run Integration -cover | grep -v "no test" | grep -v "no statement""#,
                ))?;
                Ok(())
            }
            Some("lint") => {
                bash::run_bash_and_print(String::from(
                    r#"golangci-lint run --allow-parallel-runners --fix"#,
                ))?;
                Ok(())
            }
            Some("test") => {
                bash::run_bash_and_print(String::from(
                    r#"go test ./... -cover | grep -v "no test" | grep -v "no statement""#,
                ))?;
                Ok(())
            }
            _ => {
                print!("{:?}", cmd.clone().get_matches());
                clap_cmd().print_help()?;
                Ok(())
            }
        }
    }
}
