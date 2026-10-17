//! 进程 readiness 的窄端口。
//! HTTP surface 只需要知道当前进程是否可以接收业务流量，不应直接依赖具体数据库或连接池。
//! 具体适配器负责执行只读的依赖检查，并将失败转换为不泄露下游细节的应用错误。

use super::ApplicationFuture;

/// 检查进程是否满足旧版 `/health/ready` 的最小依赖条件。
pub trait ProcessReadinessPort: Send + Sync {
    /// 执行一次只读 readiness 检查；失败时调用方只应返回有限状态码和稳定错误码。
    fn check(&self) -> ApplicationFuture<'_, ()>;
}
