use serde::Deserialize;

mod deploy;
mod osfchain;

#[derive(Default, Debug, Deserialize)]
#[serde(default)]
pub struct Config {
    pub read_signals: Vec<String>,
}

#[derive(Default, Debug, Deserialize)]
#[serde(default)]
pub struct AllConfig {
    tcn: Config,
}

#[derive(Debug, argh::FromArgs)]
/// railchain cli
struct Cli {
    /// command
    #[argh(subcommand)]
    command: Command,
}

/// command
#[derive(Debug, argh::FromArgs)]
#[argh(subcommand)]
enum Command {
    Bench(Bench),
}

fn default_host() -> String {
    "mcoms".into()
}

/// run a benchmark
#[derive(Debug, argh::FromArgs)]
#[argh(subcommand, name = "bench")]
struct Bench {
    /// program
    #[argh(option, short = 'p')]
    program: Option<String>,
    /// duration
    #[argh(option, default = "300")]
    duration: u64,
    /// series
    #[argh(option, short = 's')]
    series: Option<String>,
    /// upload
    #[argh(switch)]
    upload: bool,
    /// benchmark hosts
    #[argh(option, short = 'h', default = "default_host()")]
    hosts: String,
}

fn main() -> anyhow::Result<()> {
    let Cli { command, .. } = argh::from_env();

    match command {
        Command::Bench(bench) => {
            deploy::run(bench)?;
        }
    }

    Ok(())
}
