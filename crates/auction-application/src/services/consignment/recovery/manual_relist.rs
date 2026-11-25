//! 卖家手动重列用例：校验客户端命令 ID，产生规范摘要并交写端口。
//!
//! 保护不变量 1、6：命令 ID 必须是客户端 UUIDv4（系统派生的 UUIDv5 只属自动重列）；规范摘要只在此处
//! 产生，`request_id` 只进审计不进摘要。资格、额度、止售、季界与上代封闭都在写端同一事务内核对。

use std::sync::Arc;

use auction_domain::consignment::ListingVersion;
use auction_domain::{CommandId, UserId};
use uuid::Uuid;

use crate::ports::consignment::{
    ConsignmentError, CustodyRelistStore, RelistCommand, RelistReceipt,
    validate_listing_client_command_id, validate_listing_request_id,
};
use crate::ports::core::RequestHash;
use crate::services::consignment::recovery::encoding::canonical_relist;

/// 卖家手动重列用例；只依赖重列写端口。
pub struct ManualRelistService {
    store: Arc<dyn CustodyRelistStore>,
}

impl ManualRelistService {
    /// 注入重列写端口。
    pub fn new(store: Arc<dyn CustodyRelistStore>) -> Self {
        Self { store }
    }

    /// 卖家重列；同命令 ID 同请求由写端返回原回执，异请求为 `command_id_conflict`。
    pub async fn relist(
        &self,
        seller_user_id: UserId,
        command_id: CommandId,
        listing_id: Uuid,
        expected_version: ListingVersion,
        request_id: Option<String>,
    ) -> Result<RelistReceipt, ConsignmentError> {
        let command =
            relist_command(seller_user_id, command_id, listing_id, expected_version, request_id)?;
        self.store.relist(command).await
    }

    /// 软关闭后按原命令只读重放；摘要与首次提交同一编码，存储只读账本、不新建命令。
    pub async fn replay(
        &self,
        seller_user_id: UserId,
        command_id: CommandId,
        listing_id: Uuid,
        expected_version: ListingVersion,
        request_id: Option<String>,
    ) -> Result<Option<RelistReceipt>, ConsignmentError> {
        let command =
            relist_command(seller_user_id, command_id, listing_id, expected_version, request_id)?;
        self.store.replay_relist(command).await
    }
}

/// 校验客户端命令 ID 与请求号，组装带规范摘要的端口命令；首次提交与重放共用。
fn relist_command(
    seller_user_id: UserId,
    command_id: CommandId,
    listing_id: Uuid,
    expected_version: ListingVersion,
    request_id: Option<String>,
) -> Result<RelistCommand, ConsignmentError> {
    validate_listing_client_command_id(command_id)?;
    validate_listing_request_id(request_id.as_deref())?;
    let mut command = RelistCommand {
        command_id,
        seller_user_id,
        listing_id,
        expected_version,
        request_hash: RequestHash::new([0; 32]),
        request_id,
    };
    command.request_hash = canonical_relist(&command).request_hash();
    Ok(command)
}
