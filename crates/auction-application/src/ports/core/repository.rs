use std::future::Future;

use thiserror::Error;

use auction_domain::{Auction, AuctionId};

#[derive(Debug, Error)]
pub enum RepositoryError {
    #[error("repository operation failed: {0}")]
    Failed(String),

    /// 有界预算内没有完成依赖检查；不代表已证明仓储不可用。
    #[error("repository operation timed out")]
    Timeout,

    /// 依赖当前无法在预算内提供服务（连接池饱和、审计门禁等）；恢复后可重新判定。
    #[error("repository temporarily unavailable")]
    Unavailable,
}

pub trait AuctionRepository: Send + Sync {
    fn insert(&self, auction: Auction) -> impl Future<Output = Result<(), RepositoryError>> + Send;

    fn get(
        &self,
        id: AuctionId,
    ) -> impl Future<Output = Result<Option<Auction>, RepositoryError>> + Send;
}
