#[tokio::main]
async fn main() -> anyhow::Result<()> {
    auction_admin_api::run().await.map_err(|error| {
        // 返回类型和非零退出保持原样，不让 Termination 打印 panic 的 source 链。
        match auction_process::task_error_kind(error.as_ref()) {
            Some(kind) => anyhow::anyhow!("admin-api task failed: {kind}"),
            None => error,
        }
    })
}
