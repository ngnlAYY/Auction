//! 结算履约外部证据的稳定摘要。

use crate::{ApplicationError, RequestHash, SettlementFulfillmentSentEvidence};
use serde::Serialize;

/// 旧版 `fulfillmentProofDigest` 的 evidence 载荷。
/// 字段顺序和 `omitempty` 语义必须保持与 Go 的 `encoding/json` 一致；因此这里不复用
/// 新版的 proof DTO，也不把应用层的 command envelope 混入外部证据摘要。
#[derive(Serialize)]
struct LegacyFulfillmentEvidence<'a> {
    #[serde(rename = "external_id", skip_serializing_if = "Option::is_none")]
    external_id: Option<&'a str>,
    #[serde(rename = "world", skip_serializing_if = "Option::is_none")]
    world: Option<&'a str>,
    #[serde(rename = "subject", skip_serializing_if = "Option::is_none")]
    subject: Option<&'a str>,
    #[serde(rename = "remaining_credits", skip_serializing_if = "Option::is_none")]
    remaining_credits: Option<i64>,
    #[serde(rename = "cod_pending", skip_serializing_if = "Option::is_none")]
    cod_pending: Option<i64>,
    #[serde(rename = "equipment_ids", skip_serializing_if = "Option::is_none")]
    equipment_ids: Option<Vec<&'a str>>,
    #[serde(rename = "collected", skip_serializing_if = "Option::is_none")]
    collected: Option<bool>,
}

/// 旧版外层 proof 载荷；`json.Marshal` 按声明顺序输出这些字段。
#[derive(Serialize)]
struct LegacyFulfillmentProof<'a> {
    #[serde(rename = "schema_version")]
    schema_version: u8,
    #[serde(rename = "kind")]
    kind: &'a str,
    #[serde(rename = "delivery_id")]
    delivery_id: String,
    #[serde(rename = "status")]
    status: &'static str,
    #[serde(rename = "evidence")]
    evidence: LegacyFulfillmentEvidence<'a>,
}

/// 按旧版 Sent 载荷生成不可变摘要；claim、Balance 和观察时间不属于该摘要协议。
pub(super) fn proof_digest(
    delivery_id: uuid::Uuid,
    sent: &SettlementFulfillmentSentEvidence,
) -> Result<[u8; 32], ApplicationError> {
    let mut equipment_ids: Vec<&str> =
        sent.remaining_equipment_ids.iter().map(String::as_str).collect();
    // 只排序借用的 ID，保留旧版字符串排序和重复项，不复制 evidence 或改变输入顺序。
    equipment_ids.sort_unstable();

    let evidence = LegacyFulfillmentEvidence {
        external_id: (!sent.external_mail_id.is_empty()).then_some(sent.external_mail_id.as_str()),
        world: Some(sent.world.as_str()),
        subject: (!sent.subject.is_empty()).then_some(sent.subject.as_str()),
        remaining_credits: (sent.remaining_credits != 0).then_some(sent.remaining_credits),
        cod_pending: (sent.cod_pending != 0).then_some(sent.cod_pending),
        equipment_ids: (!equipment_ids.is_empty()).then_some(equipment_ids),
        collected: sent.collected.then_some(true),
    };
    let payload = LegacyFulfillmentProof {
        schema_version: 1,
        kind: "mooglemail_sent_collected",
        delivery_id: delivery_id.to_string(),
        status: "succeeded",
        evidence,
    };
    Ok(RequestHash::from_legacy_go_json(&payload)?.as_bytes())
}
