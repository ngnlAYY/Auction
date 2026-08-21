use anyhow::Context;
use auction_config::{AppConfig, Role};
use auction_storage::DatabaseBackend;
use clap::{Parser, Subcommand, ValueEnum};

#[derive(Debug, Parser)]
#[command(name = "auctionctl", about = "Auction Rust 运维命令")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    CheckConfig {
        #[arg(value_enum, default_value = "public")]
        role: RoleArg,
    },
    Migrate,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum RoleArg {
    Public,
    Admin,
    Worker,
}

impl From<RoleArg> for Role {
    fn from(value: RoleArg) -> Self {
        match value {
            RoleArg::Public => Self::Public,
            RoleArg::Admin => Self::Admin,
            RoleArg::Worker => Self::Worker,
        }
    }
}

fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Command::CheckConfig { role } => {
            let role: Role = role.into();
            let config = AppConfig::load(role).context("load configuration")?;
            let database_backend = config
                .database_url()
                .map(DatabaseBackend::from_url)
                .transpose()
                .context("validate database_url")?;
            println!(
                "role={} listen_addr={:?} database_backend={database_backend:?} cache_backend={}",
                config.role.name(),
                config.listen_addr,
                config.cache.backend.name(),
            );
        }
        Command::Migrate => {
            println!("database migration runner will be enabled after schema review");
        }
    }

    Ok(())
}
