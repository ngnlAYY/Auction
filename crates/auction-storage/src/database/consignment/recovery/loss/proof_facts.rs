//! 灭失证据的推导摘要：在锁内从代管事件与派发事实推导后续移动链、排除在途与证据摘要，不信任请求输入。
//!
//! 保护不变量 4、6（rulings“PIW 规划第四节”Q4、D9 §5.3）：
//! - 后续移动链取该 position 的全部代管事件（按版本），没有事件即事实不全，失败关闭；
//! - 排除在途 / 已交接：该 position 的任一合同存在结果未知的派发，或已有交接证明，都不能排除，失败关闭；
//!   能排除时，摘要覆盖全部已起写派发及其 Sent 绑定状态；
//! - 证据摘要覆盖 position、来源、观察身份、最后确定位置与上述两个摘要；同一观察再次录入时，任何一项
//!   变化都使摘要不同，由调用方判为内容矛盾。

use auction_time::Timestamp;
use sha2::{Digest, Sha256};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::recovery::withdrawal::unknown_dispatch_open;

/// 推导出的三个摘要。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct DerivedProofDigests {
    /// 完整后续移动链摘要。
    pub(super) movement: [u8; 32],
    /// 排除在途与已交接的摘要。
    pub(super) exclusion: [u8; 32],
    /// 证据内容摘要。
    pub(super) proof: [u8; 32],
}

/// 证据文本输入（摘要的一部分）。
pub(super) struct ProofText<'a> {
    pub(super) position_id: Uuid,
    pub(super) source_kind: &'a str,
    pub(super) observation_ref: &'a str,
    pub(super) last_known_ref: &'a str,
}

/// 代管事件一行：版本、种类、前后状态、证明、挂单版本与发生时刻。
type EventRow = (i64, String, Option<String>, String, Option<Uuid>, Option<i64>, Timestamp);

/// 在调用方已锁住 position 的事务内推导；事实不全或不能排除在途 / 已交接时返回 `None`。
pub(super) async fn derive_proof_digests(
    connection: &mut PgConnection,
    text: &ProofText<'_>,
) -> Result<Option<DerivedProofDigests>, WriteError> {
    let events: Vec<EventRow> = sqlx::query_as(
        "SELECT version, event_kind, from_state, to_state, proof_id, listing_version, occurred_at
         FROM public.consignment_custody_events WHERE position_id = $1 ORDER BY version",
    )
    .bind(text.position_id)
    .fetch_all(&mut *connection)
    .await?;
    if events.is_empty() {
        return Ok(None);
    }
    let handed_over: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM public.consignment_handoff_proofs WHERE position_id = $1)",
    )
    .bind(text.position_id)
    .fetch_one(&mut *connection)
    .await?;
    if handed_over || unknown_dispatch_open(connection, text.position_id).await? {
        return Ok(None);
    }
    let dispatches: Vec<(Uuid, i64, bool)> = sqlx::query_as(
        "SELECT send_attempt.delivery_id, send_attempt.fencing_version,
                EXISTS (SELECT 1 FROM public.delivery_sent_bindings AS sent
                        WHERE sent.delivery_id = send_attempt.delivery_id
                          AND sent.fencing_version = send_attempt.fencing_version)
         FROM public.consignment_contracts AS contract
         JOIN public.consignment_dispatch_rounds AS round_row
           ON round_row.contract_id = contract.contract_id
         JOIN public.delivery_send_attempts AS send_attempt
           ON send_attempt.delivery_id = round_row.delivery_id
         WHERE contract.position_id = $1
         ORDER BY send_attempt.delivery_id, send_attempt.fencing_version",
    )
    .bind(text.position_id)
    .fetch_all(&mut *connection)
    .await?;

    let mut movement = Hashed::new(b"consignment.loss-proof.movement.v1");
    movement.uuid(text.position_id);
    for (version, kind, from, to, proof_id, listing_version, occurred_at) in &events {
        movement.int(*version);
        movement.text(kind);
        movement.optional_text(from.as_deref());
        movement.text(to);
        movement.optional_uuid(*proof_id);
        movement.optional_int(*listing_version);
        movement.text(&occurred_at.to_string());
    }
    let mut exclusion = Hashed::new(b"consignment.loss-proof.exclusion.v1");
    exclusion.uuid(text.position_id);
    for (delivery_id, fence, sent) in &dispatches {
        exclusion.uuid(*delivery_id);
        exclusion.int(*fence);
        exclusion.int(i64::from(*sent));
    }
    let (movement, exclusion) = (movement.finish(), exclusion.finish());
    let mut proof = Hashed::new(b"consignment.loss-proof.v1");
    proof.uuid(text.position_id);
    proof.text(text.source_kind);
    proof.text(text.observation_ref);
    proof.text(text.last_known_ref);
    proof.bytes(&movement);
    proof.bytes(&exclusion);
    Ok(Some(DerivedProofDigests { movement, exclusion, proof: proof.finish() }))
}

/// 带域标签、逐字段定长前缀的 SHA-256；字段边界不会被拼接歧义抹去。
struct Hashed(Sha256);

impl Hashed {
    fn new(domain: &[u8]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(domain);
        hasher.update([0]);
        Self(hasher)
    }

    fn bytes(&mut self, value: &[u8]) {
        self.0.update((value.len() as u64).to_be_bytes());
        self.0.update(value);
    }

    fn text(&mut self, value: &str) {
        self.bytes(value.as_bytes());
    }

    fn uuid(&mut self, value: Uuid) {
        self.bytes(value.as_bytes());
    }

    fn int(&mut self, value: i64) {
        self.bytes(&value.to_be_bytes());
    }

    fn optional_text(&mut self, value: Option<&str>) {
        self.0.update([u8::from(value.is_some())]);
        if let Some(value) = value {
            self.text(value);
        }
    }

    fn optional_uuid(&mut self, value: Option<Uuid>) {
        self.0.update([u8::from(value.is_some())]);
        if let Some(value) = value {
            self.uuid(value);
        }
    }

    fn optional_int(&mut self, value: Option<i64>) {
        self.0.update([u8::from(value.is_some())]);
        if let Some(value) = value {
            self.int(value);
        }
    }

    fn finish(self) -> [u8; 32] {
        self.0.finalize().into()
    }
}
