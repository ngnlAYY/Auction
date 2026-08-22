//! 应用命令端口。
//!
//! HTTP、Worker 和其他入口只依赖这些窄端口，不直接持有具体数据库事务或网络客户端。

use std::{future::Future, pin::Pin};

use auction_domain::CompetitionPlan;

use crate::{ApplicationError, CreateProxyCommand, LowerProxyCommand, PlaceBidCommand};

/// 应用用例异步返回值，允许 HTTP state 持有动态端口而不暴露具体存储类型。
pub type ApplicationFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, ApplicationError>> + Send + 'a>>;

/// Public 竞价命令端口。
pub trait CompetitionCommandPort: Send + Sync {
    /// 执行普通竞价；幂等、锁和领域计划由应用服务与存储事务共同保证。
    fn place_bid(&self, command: PlaceBidCommand) -> ApplicationFuture<'_, CompetitionPlan>;

    /// 创建代理授权；授权初始竞价与授权事实在同一事务中提交。
    fn create_proxy(&self, command: CreateProxyCommand) -> ApplicationFuture<'_, CompetitionPlan>;

    /// 降低活动代理上限；转换事实与授权快照在同一事务中提交。
    fn lower_proxy(&self, command: LowerProxyCommand) -> ApplicationFuture<'_, CompetitionPlan>;
}
