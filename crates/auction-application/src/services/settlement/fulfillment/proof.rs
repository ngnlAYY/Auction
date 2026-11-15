//! 结算履约外部证据的稳定摘要。

use serde::Serialize;

use crate::errors::ApplicationError;
use crate::ports::core::RequestHash;
use crate::ports::settlement::SettlementFulfillmentSentEvidence;

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

/// 按已发布的 Sent 证明格式生成不可变摘要，字节兼容、不可修改，由
/// `proof_digest_matches_frozen_vectors` 固定；claim、Balance 与观察时间不属于该摘要协议。
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

#[cfg(test)]
mod tests {
    use auction_domain::SettlementWorld;
    use chrono::{TimeZone, Utc};
    use uuid::Uuid;

    use super::proof_digest;
    use crate::ports::settlement::SettlementFulfillmentSentEvidence;

    fn digest(evidence: &SettlementFulfillmentSentEvidence) -> String {
        let delivery_id =
            Uuid::parse_str("8b6d4f2a-0c1e-4d3b-a5f7-9e2c4a6b8d0f").expect("测试 UUID 必须合法");
        proof_digest(delivery_id, evidence)
            .expect("证明摘要必须可编码")
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    /// Sent 证明摘要随履约结果持久化：装备 ID 排序后参与，零值与空值字段省略，收件人与观察
    /// 时间不属于摘要协议。向量由与 v0.1.4 发布版逐字相同的实现生成。
    #[test]
    fn proof_digest_matches_frozen_vectors() {
        let full = SettlementFulfillmentSentEvidence {
            external_mail_id: "9000001".to_owned(),
            world: SettlementWorld::Isekai,
            recipient: "Sample".to_owned(),
            subject: "Auction delivery".to_owned(),
            remaining_credits: 1_250_000,
            cod_pending: 30_000,
            remaining_equipment_ids: vec!["100000002".to_owned(), "100000001".to_owned()],
            collected: true,
            observed_at: Utc.with_ymd_and_hms(2026, 11, 14, 12, 0, 0).single().expect("测试时间"),
        };
        let minimal = SettlementFulfillmentSentEvidence {
            external_mail_id: String::new(),
            world: SettlementWorld::Main,
            subject: String::new(),
            remaining_credits: 0,
            cod_pending: 0,
            remaining_equipment_ids: Vec::new(),
            collected: false,
            ..full.clone()
        };
        assert_eq!(
            [digest(&full), digest(&minimal)],
            [
                "cad1758195efd5f573fbcb522e82eb322a3478df94796420ba63ceb7e2715034",
                "f2b73ec2a3e3e1847680ad742e7d8a2ea55ae319cd659824547dbdbb202e506d",
            ]
        );
        let mut unordered = full.clone();
        unordered.remaining_equipment_ids.reverse();
        unordered.recipient = "Other".to_owned();
        unordered.observed_at = Utc::now();
        assert_eq!(digest(&unordered), digest(&full));
    }
}
