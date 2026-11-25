//! 寄售首轮派发邮件内容来源的 Public 装配：把存储的首轮请求原样交给 integrations 的唯一渲染实现
//! [`render_first_dispatch`]（Worker 补寄新轮与发信前核对用的是同一实现）；直接撤回的撤回件首轮经
//! [`render_withdrawal_dispatch`] 冻结。
//!
//! 本文件只做类型映射，不含渲染、金额或配对规则；任何不一致都由渲染实现返回 `Unavailable`，整笔
//! 购买回滚为 503，调用方以同命令 ID 重试（不变量 4、6）。

use std::sync::Arc;

use auction_application::consignment::ConsignmentError;
use auction_integrations::mooglemail::{
    FirstDispatchFacts, WithdrawalDispatchFacts, render_first_dispatch, render_withdrawal_dispatch,
};
use auction_storage::{
    DepositRepository, FirstDispatchContent, FirstDispatchContentSource, FirstDispatchRequest,
    WithdrawalDispatchRequest,
};

/// 给收件仓库注入撤回件首轮的冻结内容来源（与购买首轮同一渲染实现）：直接撤回与 Withdrawal 同提首轮
/// （PIWX）。
pub(super) fn with_withdrawal_content(repository: DepositRepository) -> Arc<DepositRepository> {
    Arc::new(repository.with_dispatch_content(Arc::new(FrozenFirstDispatchContent)))
}

/// 用代码冻结模板渲染首轮内容的来源；无状态。
pub(super) struct FrozenFirstDispatchContent;

impl FirstDispatchContentSource for FrozenFirstDispatchContent {
    fn content(
        &self,
        request: &FirstDispatchRequest<'_>,
    ) -> Result<FirstDispatchContent, ConsignmentError> {
        let mail = render_first_dispatch(&FirstDispatchFacts {
            world: request.world,
            price: request.price,
            round_ref: request.round_ref,
            payment_deadline_at: request.payment_deadline_at,
            item_name: request.item_name,
            canonical_eid: request.canonical_eid,
            payment: request.payment,
        })?;
        Ok(FirstDispatchContent {
            template: mail.template.to_owned(),
            subject: mail.subject.clone(),
            body_digest: mail.body_digest(),
            equipment_ids: mail.equipment_ids,
        })
    }

    fn withdrawal_content(
        &self,
        request: &WithdrawalDispatchRequest<'_>,
    ) -> Result<FirstDispatchContent, ConsignmentError> {
        let mail = render_withdrawal_dispatch(&WithdrawalDispatchFacts {
            world: request.world,
            round_ref: request.round_ref,
            item_name: request.item_name,
            canonical_eid: request.canonical_eid,
            obligation: request.obligation,
        })?;
        Ok(FirstDispatchContent {
            template: mail.template.to_owned(),
            subject: mail.subject.clone(),
            body_digest: mail.body_digest(),
            equipment_ids: mail.equipment_ids,
        })
    }
}
