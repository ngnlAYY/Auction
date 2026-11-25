//! 购买与 v2 改价用例：校验客户端命令 ID 与请求号、计算规范摘要后交给存储端口。

use std::sync::Arc;

use auction_domain::consignment::funding::FundingMode;
use auction_domain::consignment::purchase::{PurchaseFunding, PurchaseInput, RepriceInput};
use auction_domain::consignment::{FundingAccountingMode, SaleFundingFreeze};
use auction_domain::{CommandId, UserId};

use super::encoding::{canonical_purchase, canonical_reprice_v2, request_hash};
use crate::ports::consignment::{
    ConsignmentError, PurchaseCommand, PurchaseOutcome, PurchaseStore, RepriceV2Command,
    RepriceV2Outcome, RepriceV2Store, validate_listing_client_command_id,
    validate_listing_request_id,
};
use crate::ports::settlement::consignment_funding::FundingPreflightFact;

/// 寄售购买与 v2 改价写用例。
pub struct PurchaseCommandService {
    purchases: Arc<dyn PurchaseStore>,
    reprices: Arc<dyn RepriceV2Store>,
}

impl PurchaseCommandService {
    /// 注入两个窄端口。
    pub fn new(purchases: Arc<dyn PurchaseStore>, reprices: Arc<dyn RepriceV2Store>) -> Self {
        Self { purchases, reprices }
    }

    /// 买家购买；非 UUIDv4 的命令 ID 与非法请求号在进入存储前失败，不占账本。
    pub async fn purchase(
        &self,
        buyer_user_id: UserId,
        command_id: CommandId,
        input: PurchaseInput,
        request_id: Option<String>,
    ) -> Result<PurchaseOutcome, ConsignmentError> {
        let command = purchase_command(buyer_user_id, command_id, input, request_id)?;
        self.purchases.purchase(command).await
    }

    /// 只重放：关闭寄售后已接受的购买仍须回原结果（不变量 1）。账本没有该命令返回 `None`，
    /// 同 ID 异请求为 `CommandIdConflict`；不受理任何新命令。
    pub async fn replay_purchase(
        &self,
        buyer_user_id: UserId,
        command_id: CommandId,
        input: PurchaseInput,
        request_id: Option<String>,
    ) -> Result<Option<PurchaseOutcome>, ConsignmentError> {
        let command = purchase_command(buyer_user_id, command_id, input, request_id)?;
        self.purchases.replay_purchase(command).await
    }

    /// 卖家 v2 改价，校验同购买；发起卖家取自 `input.seller_user_id`，与判定所用同源。
    pub async fn reprice(
        &self,
        command_id: CommandId,
        input: RepriceInput,
        request_id: Option<String>,
    ) -> Result<RepriceV2Outcome, ConsignmentError> {
        validate_listing_client_command_id(command_id)?;
        validate_listing_request_id(request_id.as_deref())?;
        let seller_user_id = input.seller_user_id;
        let canonical = canonical_reprice_v2(
            seller_user_id,
            input.listing_id,
            input.expected_version,
            input.price,
        );
        self.reprices
            .reprice(RepriceV2Command {
                command_id,
                request_hash: request_hash(&canonical),
                seller_user_id,
                input,
                request_id,
            })
            .await
    }
}

/// 校验并组装购买命令；购买与只重放共用，摘要口径只有这一处。
fn purchase_command(
    buyer_user_id: UserId,
    command_id: CommandId,
    input: PurchaseInput,
    request_id: Option<String>,
) -> Result<PurchaseCommand, ConsignmentError> {
    validate_listing_client_command_id(command_id)?;
    validate_listing_request_id(request_id.as_deref())?;
    let canonical = canonical_purchase(
        buyer_user_id,
        input.listing_id,
        input.expected_version,
        input.expected_price,
    );
    Ok(PurchaseCommand {
        command_id,
        request_hash: request_hash(&canonical),
        buyer_user_id,
        input,
        request_id,
    })
}

/// 把预检事实映射为购买判定事实；`Ready` 的冻结值随事实进入判定与 Sale，不得另读第二次。
pub fn purchase_funding(fact: &FundingPreflightFact) -> PurchaseFunding {
    match fact {
        FundingPreflightFact::Ready {
            uid,
            mode,
            boundary_at,
            boundary_source_id,
            account_version,
        } => PurchaseFunding::Ready(SaleFundingFreeze {
            uid: uid.clone(),
            mode: match mode {
                FundingMode::StrictCash => FundingAccountingMode::StrictCash,
                FundingMode::RecordOnly => FundingAccountingMode::RecordOnly,
            },
            boundary_at: *boundary_at,
            boundary_source_id: *boundary_source_id,
            account_version: *account_version,
        }),
        FundingPreflightFact::Unavailable(_) => PurchaseFunding::Unavailable,
    }
}
