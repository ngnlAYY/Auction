//! 可选单文件服务的薄装配入口。

#[cfg(all(feature = "postgres", target_os = "linux"))]
mod bootstrap;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    #[cfg(all(feature = "postgres", target_os = "linux"))]
    {
        bootstrap::run().await
    }
    #[cfg(not(all(feature = "postgres", target_os = "linux")))]
    anyhow::bail!("auction-server requires Linux and the postgres feature")
}
