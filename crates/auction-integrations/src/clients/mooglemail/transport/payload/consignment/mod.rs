//! 寄售发送邮件的变量校验与代码冻结正文渲染。
//!
//! 七个寄售模板不在运营可编辑的模板目录里：正文由 [`templates`] 冻结，变量闭集按模板固定。
//! 变量里的 `dispatch_ref` 首派也必须存在，并写在正文末行；系统退件原样带回，据此绑定派发轮。
//! 本模块只渲染，不决定资产、金额或状态（通知模板不授资产权）。

use std::collections::BTreeMap;

use auction_application::consignment::consignment_send_template_kind;
use auction_application::public::format_credits;
use auction_application::settlement::{ConsignmentSubjectKind, canonical_consignment_subject};
use auction_domain::SettlementId;
use uuid::Uuid;

use crate::clients::mooglemail::forms::valid_body;
use crate::clients::mooglemail::protocol::valid_line;

mod dispatch_mail;
mod templates;
#[cfg(test)]
mod vectors_test;

pub use dispatch_mail::{
    DownstreamDispatchFacts, FirstDispatchFacts, RenderedDispatchMail, WithdrawalDispatchFacts,
    render_downstream_dispatch, render_first_dispatch, render_frozen_dispatch,
    render_withdrawal_dispatch,
};

/// 校验后的模板变量；字段只在通过校验后才会构造。
struct Variables<'a> {
    reference: &'a str,
    contract: &'a str,
    dispatch_ref: &'a str,
    item_name: &'a str,
    amount: Option<String>,
    payment_deadline: Option<&'a str>,
}

/// 渲染寄售发送邮件，返回（规范主题，正文）。模板名不是寄售模板、变量键集合不是该模板的闭集、
/// 任一变量非规范形式时返回 `None`（失败关闭，不补默认值）。
pub fn render_consignment_send(
    template: &str,
    variables: &BTreeMap<String, String>,
) -> Option<(String, String)> {
    let kind = consignment_send_template_kind(template)?;
    let variables = validated_variables(kind, variables)?;
    let obligation_id = SettlementId::from_uuid(Uuid::parse_str(variables.reference).ok()?);
    let subject = canonical_consignment_subject(kind, obligation_id);
    let body = templates::render_body(kind, &variables);
    (valid_line(&subject, 512, false) && valid_body(&body)).then_some((subject, body))
}

/// 该种类的变量键闭集（含 `dispatch_ref`）。
fn variable_names(kind: ConsignmentSubjectKind) -> &'static [&'static str] {
    match kind {
        ConsignmentSubjectKind::EquipmentCod
        | ConsignmentSubjectKind::PaymentCarrier
        | ConsignmentSubjectKind::PaymentReminder => {
            &["amount", "contract", "dispatch_ref", "item_name", "payment_deadline", "reference"]
        }
        ConsignmentSubjectKind::SellerPayout | ConsignmentSubjectKind::BuyerRefund => {
            &["amount", "contract", "dispatch_ref", "item_name", "reference"]
        }
        ConsignmentSubjectKind::EquipmentHandoff | ConsignmentSubjectKind::EquipmentWithdrawal => {
            &["contract", "dispatch_ref", "item_name", "reference"]
        }
    }
}

fn validated_variables(
    kind: ConsignmentSubjectKind,
    variables: &BTreeMap<String, String>,
) -> Option<Variables<'_>> {
    let names = variable_names(kind);
    if variables.len() != names.len() || !names.iter().all(|name| variables.contains_key(*name)) {
        return None;
    }
    let text = |name: &str| variables.get(name).map(String::as_str);
    let reference = text("reference").filter(|value| canonical_uuid(value))?;
    let contract = text("contract").filter(|value| canonical_uuid(value))?;
    let dispatch_ref = text("dispatch_ref").filter(|value| canonical_uuid(value))?;
    let item_name = text("item_name").filter(|value| valid_line(value, 512, false))?;
    let amount = match text("amount") {
        Some(value) => Some(format_credits(positive_amount(value)?)),
        None => None,
    };
    let payment_deadline = match text("payment_deadline") {
        Some(value) if canonical_deadline(value) => Some(value),
        Some(_) => return None,
        None => None,
    };
    Some(Variables { reference, contract, dispatch_ref, item_name, amount, payment_deadline })
}

fn canonical_uuid(value: &str) -> bool {
    Uuid::parse_str(value).ok().is_some_and(|id| !id.is_nil() && id.to_string() == value)
}

fn positive_amount(value: &str) -> Option<i64> {
    value.parse::<i64>().ok().filter(|amount| *amount > 0 && amount.to_string() == value)
}

/// 期限只接受规范 UTC 秒精度文本，避免把本地时区或可读日期当成冻结事实。
fn canonical_deadline(value: &str) -> bool {
    auction_time::Timestamp::parse_from_rfc3339(value).ok().is_some_and(|deadline| {
        deadline.to_rfc3339_opts(auction_time::SecondsFormat::Secs, true) == value
    })
}
