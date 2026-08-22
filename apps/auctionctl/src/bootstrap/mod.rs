use anyhow::Context;
use auction_config::{AppConfig, Role};
use auction_storage::DatabaseBackend;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
use auction_storage::DatabaseStore;
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

pub async fn run() -> anyhow::Result<()> {
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
            migrate().await?;
        }
    }

    Ok(())
}

async fn migrate() -> anyhow::Result<()> {
    let config = AppConfig::load(Role::Public).context("load configuration for migration")?;
    let database_url = config.database_url().context("database_url is required for migration")?;

    #[cfg(any(feature = "postgres", feature = "sqlite"))]
    {
        let store =
            DatabaseStore::connect(database_url).await.context("connect database for migration")?;
        store.migrate().await.context("run database migrations")?;
        println!("database migrations applied for {}", store.backend());
        Ok(())
    }

    #[cfg(not(any(feature = "postgres", feature = "sqlite")))]
    {
        let _ = database_url;
        anyhow::bail!("auctionctl was built without a database feature; enable postgres or sqlite")
    }
}
