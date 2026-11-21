//! 本人改价、本人取消和固定 Admin 下架的命令编排。

use std::sync::Arc;

use auction_domain::consignment::{ListingPrice, ListingVersion};
use auction_domain::{CommandId, UserId};
use uuid::Uuid;

use crate::errors::{ApplicationError, SessionError};
use crate::ports::consignment::{
    AdminDelistListingCommand, ConsignmentError, ListingClosed, ListingCommandStore,
    ListingDelistReason, ListingOperation, ListingPriceReceipt, RepriceListingCommand,
    SellerListingTarget, WithdrawListingCommand,
};
use crate::ports::public::{AdminSessionIdentity, PublicSessionIdentity};
use crate::services::consignment::encoding::ListingCommandEncoding;
use crate::services::sessions::validate_admin_actor;

/// 本人和运营写入的用例。身份由各自的认证守卫提供。
pub struct ManageListingService {
    store: Arc<dyn ListingCommandStore>,
    reprice_enabled: bool,
}

impl ManageListingService {
    /// 注入命令仓储与首次改价的本地准入。关闭时仓储仍须先仲裁旧命令。
    pub fn new(store: Arc<dyn ListingCommandStore>, reprice_enabled: bool) -> Self {
        Self { store, reprice_enabled }
    }

    /// 改价或取得原回执。所有权、强制终止、版本和期限以同一个存储事务为准。
    pub async fn reprice(
        &self,
        seller: &PublicSessionIdentity,
        command_id: CommandId,
        listing_id: Uuid,
        expected_version: u64,
        price: i64,
    ) -> Result<ListingPriceReceipt, ConsignmentError> {
        let target = seller_target(seller, command_id, listing_id, expected_version)?;
        let price = ListingPrice::new(price)?;
        let mut input = ListingCommandEncoding::target_version(
            ListingOperation::Reprice,
            listing_id,
            target.expected_version,
        );
        input.bytes(&price.value().to_be_bytes());
        // 不变量 1、2、3、6、8：关闭开关不入摘要，仓储先仲裁再决定首次准入。
        self.store
            .reprice(RepriceListingCommand {
                request_hash: input.request_hash(),
                target,
                price,
                enabled: self.reprice_enabled,
            })
            .await?
            .into_result()
    }

    /// 取消本人挂单，或由同一事务收口强制终止。
    pub async fn withdraw(
        &self,
        seller: &PublicSessionIdentity,
        command_id: CommandId,
        listing_id: Uuid,
        expected_version: u64,
    ) -> Result<ListingClosed, ConsignmentError> {
        let target = seller_target(seller, command_id, listing_id, expected_version)?;
        let input = ListingCommandEncoding::target_version(
            ListingOperation::Withdraw,
            listing_id,
            target.expected_version,
        );
        // 不变量 1、2、3、5、6、7、8：取消与撤销解析、退休信封、回执和审计同提交。
        self.store
            .withdraw(WithdrawListingCommand { request_hash: input.request_hash(), target })
            .await?
            .into_result()
    }

    /// 用认证守卫返回的固定运营身份下架。私有说明进入规范摘要和存储命令。
    pub async fn admin_delist(
        &self,
        admin: &AdminSessionIdentity,
        command_id: CommandId,
        listing_id: Uuid,
        expected_version: u64,
        reason: String,
    ) -> Result<ListingClosed, ConsignmentError> {
        validate_admin_actor(&admin.actor).map_err(|error| match error {
            ApplicationError::Session(SessionError::InvalidCredentials) => {
                ConsignmentError::InvalidListingRequest
            }
            error => ConsignmentError::read_dependency_failure(error),
        })?;
        let expected_version = validate_target(command_id, listing_id, expected_version)?;
        let reason = ListingDelistReason::new(reason)?;
        let mut input = ListingCommandEncoding::target_version(
            ListingOperation::AdminDelist,
            listing_id,
            expected_version,
        );
        input.text(reason.as_str());
        // 不变量 1、2、3、6、7、8：原操作的状态和拒绝都由事务决定，运营说明不写日志。
        self.store
            .admin_delist(AdminDelistListingCommand {
                admin_actor: admin.actor.clone(),
                request_hash: input.request_hash(),
                command_id,
                listing_id,
                expected_version,
                reason,
            })
            .await?
            .into_result()
    }
}

fn seller_target(
    seller: &PublicSessionIdentity,
    command_id: CommandId,
    listing_id: Uuid,
    expected_version: u64,
) -> Result<SellerListingTarget, ConsignmentError> {
    if seller.user_id.is_nil() {
        return Err(ConsignmentError::InvalidListingRequest);
    }
    Ok(SellerListingTarget {
        command_id,
        listing_id,
        seller_user_id: UserId::from_uuid(seller.user_id),
        expected_version: validate_target(command_id, listing_id, expected_version)?,
    })
}

fn validate_target(
    command_id: CommandId,
    listing_id: Uuid,
    expected_version: u64,
) -> Result<ListingVersion, ConsignmentError> {
    if command_id.is_nil() {
        return Err(ConsignmentError::InvalidCommandId);
    }
    if listing_id.is_nil() {
        return Err(ConsignmentError::InvalidListingId);
    }
    Ok(ListingVersion::new(expected_version)?)
}
