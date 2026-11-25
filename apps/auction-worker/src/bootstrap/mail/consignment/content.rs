//! 寄售派发邮件内容来源的 Worker 装配：补寄新轮在存储备好事务里经此渲染，原样交给 integrations 的
//! 唯一渲染实现 [`render_first_dispatch`]（与 Public 购买冻结首轮同一实现）；取得事务里兑现撤回意愿时，
//! 撤回件首轮经 [`render_withdrawal_dispatch`] 冻结；下游首轮（isekai 原件交割、卖款、退款）经
//! [`render_downstream_dispatch`] 冻结。
//!
//! 本文件只做类型映射，不含渲染、金额或配对规则；任何不一致都由渲染实现返回 `Unavailable`，补寄新轮
//! 不备好、不建投递（不变量 4、6）。

use auction_application::consignment::ConsignmentError;
use auction_integrations::mooglemail::{
    DownstreamDispatchFacts, FirstDispatchFacts, WithdrawalDispatchFacts,
    render_downstream_dispatch, render_first_dispatch, render_withdrawal_dispatch,
};
use auction_storage::{
    DownstreamDispatchContentSource, DownstreamDispatchRequest, FirstDispatchContent,
    FirstDispatchContentSource, FirstDispatchRequest, WithdrawalDispatchRequest,
};

/// 用代码冻结模板渲染首轮内容的来源；无状态。
pub struct FrozenFirstDispatchContent;

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

impl DownstreamDispatchContentSource for FrozenFirstDispatchContent {
    fn downstream_content(
        &self,
        request: &DownstreamDispatchRequest<'_>,
    ) -> Result<FirstDispatchContent, ConsignmentError> {
        let mail = render_downstream_dispatch(&DownstreamDispatchFacts {
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
