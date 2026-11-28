//! 原信观察的受控持久化：原信 K、证据会话绑定、清单 JSON 与代管实物身份。
//!
//! 观察行只追加；相同观察身份重放必须逐字段一致。装备 URL、key、预约码与正文不进入
//! 本表的安全投影。

use std::collections::BTreeMap;

use auction_domain::SettlementWorld;
use auction_domain::consignment::OriginalMailKey;
use auction_domain::consignment::deposit::{
    EnvelopeItemObservation, EnvelopeObservation, LegacyAuctionOwner, LegacyReturnEvidence,
    SenderIdentity,
};
use auction_time::Timestamp;
use serde_json::Value;
use sqlx::PgConnection;
use uuid::Uuid;

use super::identity::{MailKeyColumns, parse_world};
use super::{invariant, invariant_decoding};
use crate::database::consignment::commands::errors::WriteError;

mod facts;
mod write;

pub(crate) use facts::parse_manifest;
use facts::{DerivedItem, parse_derived_facts};
pub(crate) use write::record_observation_in_tx;
pub(crate) use write::write_observation_in_tx;

/// 4D1 绑定观察中无法从安全投影反推的原始字段。
#[derive(Debug, Clone)]
pub(crate) struct RawObservation<'a> {
    /// 稳定原信键。
    pub(crate) mail_key: &'a OriginalMailKey,
    /// 认证证据会话。
    pub(crate) session_id: Uuid,
    /// 邮箱键。
    pub(crate) mailbox_key: &'a str,
    /// 远端观察身份。
    pub(crate) remote_observation_id: &'a str,
    /// 观察时刻。
    pub(crate) observed_at: Timestamp,
    /// 解析器版本。
    pub(crate) parser_version: &'a str,
    /// 受控证据引用。
    pub(crate) evidence_ref: Option<&'a str>,
    /// 主题摘要。
    pub(crate) subject_digest: [u8; 32],
    /// 正文摘要。
    pub(crate) body_digest: [u8; 32],
    /// 完整发件人显示名。
    pub(crate) sender: Option<&'a str>,
    /// 完整收件人显示名。
    pub(crate) recipient: Option<&'a str>,
    /// 原始附件数；读不出时为空。
    pub(crate) raw_count: Option<u32>,
    /// 领域观察；清单与逐件顺序由它产生。
    pub(crate) observation: &'a EnvelopeObservation,
    /// 系统 RTS 上按旧路径口径拆出的退件证据。
    pub(crate) legacy_evidence: LegacyReturnEvidence,
    /// 原始收件分钟文本。
    pub(crate) received_minute_raw: &'a str,
    /// 原始收件时区。
    pub(crate) received_timezone: &'a str,
    /// 首次收件时刻。
    pub(crate) first_seen_at: Timestamp,
    /// 冻结季。
    pub(crate) season_window_id: Option<Uuid>,
}

/// 从数据库读回的安全观察投影。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StoredObservation {
    /// 观察 UUID。
    pub(crate) observation_id: Uuid,
    /// 世界。
    pub(crate) world: SettlementWorld,
    /// 邮箱键；仅用于重读定位。
    pub(crate) mailbox_key: String,
    /// 证据会话。
    pub(crate) session_id: Uuid,
    /// 会话绑定摘要。
    pub(crate) session_binding_digest: [u8; 32],
    /// 清单摘要。
    pub(crate) manifest_digest: [u8; 32],
    /// 主题摘要。
    pub(crate) subject_digest: [u8; 32],
    /// 正文摘要。
    pub(crate) body_digest: [u8; 32],
    /// 解析器版本。
    pub(crate) parser_version: String,
    /// 主题是否为 `consign`。
    pub(crate) subject_is_consign: bool,
    /// 发件人身份关系。
    pub(crate) sender_identity: SenderIdentity,
    /// 列表与详情清单一致。
    pub(crate) list_detail_consistent: bool,
    /// 重复原文 EID。
    pub(crate) duplicate_raw_eid: bool,
    /// 规范别名歧义。
    pub(crate) canonical_alias_ambiguous: bool,
    /// 存在未知附件。
    pub(crate) unknown_attachment: bool,
    /// 疑似系统 RTS。
    pub(crate) system_rts: bool,
    /// 已识别的旧拍卖 owner。
    pub(crate) legacy_auction_owner: Option<LegacyAuctionOwner>,
    /// 系统 RTS 上按旧路径口径拆出的退件证据。
    pub(crate) legacy_return_evidence: LegacyReturnEvidence,
    /// 观察时刻。
    pub(crate) observed_at: Timestamp,
    /// 发件人显示名。
    pub(crate) sender: Option<String>,
    /// 收件人显示名。
    pub(crate) recipient: Option<String>,
    /// 服务器允许原样退回。
    pub(crate) returnable: Option<bool>,
    /// 服务器允许取得。
    pub(crate) takeable: Option<bool>,
    /// Credits。
    pub(crate) credits: Option<i64>,
    /// COD。
    pub(crate) cod: Option<i64>,
    /// 原始附件数。
    pub(crate) raw_count: Option<i32>,
    /// 观察时确定的资产数；0 表示确定为空，`None` 表示未知。
    pub(crate) asset_count: Option<u32>,
    /// 清单逐件投影。
    pub(crate) items: Vec<EnvelopeItemObservation>,
    /// 冻结季。
    pub(crate) season_window_id: Option<Uuid>,
    /// 受控证据引用。
    pub(crate) evidence_ref: Option<String>,
    /// 原始收件分钟文本。
    pub(crate) received_minute_raw: String,
    /// 首次收件时刻。
    pub(crate) received_at: Timestamp,
}

/// 观察行的唯一写入来源：首次观察、隔离复制与追加观察都经这里。
pub(crate) enum ObservationWrite<'a> {
    /// 由 4D1 原始观察首次写入。
    Fresh {
        /// 原始观察字段。
        raw: &'a RawObservation<'a>,
    },
    /// 把既有观察行按新身份复制为隔离内观察；派生事实与清单原样复制。
    Copy {
        /// 源观察行。
        source_id: Uuid,
        /// 新观察行。
        observation_id: Uuid,
        /// 隔离内远端观察身份。
        remote_observation_id: &'a str,
        /// 隔离 UUID。
        quarantine_id: Uuid,
        /// 写入时的隔离版本。
        quarantine_version: u64,
        /// 本次重读识别出的旧拍卖 owner；只覆盖派生事实中的该键。
        legacy_auction_owner: Option<LegacyAuctionOwner>,
    },
}

/// 按 K 与观察 UUID 读取安全投影。
pub(crate) async fn load_observation(
    connection: &mut PgConnection,
    key: &OriginalMailKey,
    observation_id: Uuid,
) -> Result<Option<StoredObservation>, WriteError> {
    let columns = MailKeyColumns::from_key(key);
    let row = sqlx::query(
        "SELECT o.id, o.world_id, o.mailbox_key, o.session_id, s.binding_digest,
                o.manifest_digest, o.subject_digest, o.body_digest, o.parser_version,
                o.derived_facts,
                o.observed_at, o.sender, o.recipient, o.returnable, o.takeable, o.credits,
                o.cod, o.raw_count, o.manifest, o.season_window_id, o.evidence_ref,
                original.received_minute_raw, original.first_seen_at
         FROM public.mail_envelope_observations AS o
         JOIN public.mail_evidence_sessions AS s ON s.id = o.session_id
         JOIN public.mail_originals AS original
           ON original.bot_uid = o.bot_uid AND original.world_id = o.world_id
          AND original.external_mid = o.external_mid
         WHERE o.bot_uid = $1 AND o.world_id = $2 AND o.external_mid = $3 AND o.id = $4",
    )
    .bind(columns.bot_uid)
    .bind(columns.world_id)
    .bind(columns.external_mid)
    .bind(observation_id)
    .fetch_optional(&mut *connection)
    .await?;
    let Some(row) = row else { return Ok(None) };
    let world = parse_world(columns.world_id)?;
    let manifest: Value = sqlx::Row::try_get(&row, "manifest")?;
    let derived_value: Value = sqlx::Row::try_get(&row, "derived_facts")?;
    let derived = parse_derived_facts(&derived_value)?;
    let derived_items: BTreeMap<u32, DerivedItem> =
        derived.items.iter().map(|item| (item.ordinal, item.clone())).collect();
    let mut items = parse_manifest(&manifest, world)?;
    for item in &mut items {
        if let Some(fact) = derived_items.get(&item.ordinal) {
            item.world = Some(fact.world);
            item.tradeable = fact.tradeable;
            item.history_ok = fact.history_ok;
        }
    }
    let binding: &[u8] = sqlx::Row::try_get(&row, "binding_digest")?;
    let manifest_digest: &[u8] = sqlx::Row::try_get(&row, "manifest_digest")?;
    let subject_digest: &[u8] = sqlx::Row::try_get(&row, "subject_digest")?;
    let body_digest: &[u8] = sqlx::Row::try_get(&row, "body_digest")?;
    let binding = digest(binding, "consignment_observation_session_digest")?;
    let manifest_digest = digest(manifest_digest, "consignment_observation_manifest_digest")?;
    let subject_digest = digest(subject_digest, "consignment_observation_subject_digest")?;
    let body_digest = digest(body_digest, "consignment_observation_body_digest")?;
    Ok(Some(StoredObservation {
        observation_id: sqlx::Row::try_get(&row, "id")?,
        world,
        mailbox_key: sqlx::Row::try_get(&row, "mailbox_key")?,
        session_id: sqlx::Row::try_get(&row, "session_id")?,
        session_binding_digest: binding,
        manifest_digest,
        subject_digest,
        body_digest,
        parser_version: sqlx::Row::try_get(&row, "parser_version")?,
        subject_is_consign: derived.subject_is_consign,
        sender_identity: derived.sender_identity,
        list_detail_consistent: derived.list_detail_consistent,
        duplicate_raw_eid: derived.duplicate_raw_eid,
        canonical_alias_ambiguous: derived.canonical_alias_ambiguous,
        unknown_attachment: derived.unknown_attachment,
        system_rts: derived.system_rts,
        legacy_auction_owner: derived.legacy_auction_owner,
        legacy_return_evidence: derived.legacy_return_evidence,
        observed_at: sqlx::Row::try_get(&row, "observed_at")?,
        sender: sqlx::Row::try_get(&row, "sender")?,
        recipient: sqlx::Row::try_get(&row, "recipient")?,
        returnable: sqlx::Row::try_get(&row, "returnable")?,
        takeable: sqlx::Row::try_get(&row, "takeable")?,
        credits: sqlx::Row::try_get(&row, "credits")?,
        cod: sqlx::Row::try_get(&row, "cod")?,
        raw_count: sqlx::Row::try_get(&row, "raw_count")?,
        asset_count: derived.asset_count,
        items,
        season_window_id: sqlx::Row::try_get(&row, "season_window_id")?,
        evidence_ref: sqlx::Row::try_get(&row, "evidence_ref")?,
        received_minute_raw: derived.received_minute_raw,
        received_at: sqlx::Row::try_get(&row, "first_seen_at")?,
    }))
}

fn digest(value: &[u8], reason: &'static str) -> Result<[u8; 32], WriteError> {
    <[u8; 32]>::try_from(value).map_err(invariant_decoding(reason))
}

/// 读取隔离当前最新观察；重读定位与来源投影只取观察行事实。
pub(crate) async fn load_latest_quarantine_observation(
    connection: &mut PgConnection,
    key: &OriginalMailKey,
    quarantine_id: Uuid,
) -> Result<Option<StoredObservation>, WriteError> {
    let observation_id: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM public.mail_envelope_observations
         WHERE quarantine_id = $1
         ORDER BY quarantine_version DESC, observed_at DESC, id DESC LIMIT 1",
    )
    .bind(quarantine_id)
    .fetch_optional(&mut *connection)
    .await?;
    match observation_id {
        Some(observation_id) => load_observation(connection, key, observation_id).await,
        None => Ok(None),
    }
}

/// 读取某条观察的旧 owner 派生事实；读端口与已解除详情共用。
pub(crate) async fn legacy_owner_for_observation(
    connection: &mut PgConnection,
    observation_id: Uuid,
) -> Result<Option<LegacyAuctionOwner>, WriteError> {
    let value: Option<Value> = sqlx::query_scalar(
        "SELECT derived_facts FROM public.mail_envelope_observations WHERE id = $1",
    )
    .bind(observation_id)
    .fetch_optional(&mut *connection)
    .await?;
    value
        .as_ref()
        .map(parse_derived_facts)
        .transpose()
        .map(|derived| derived.and_then(|facts| facts.legacy_auction_owner))
}

/// 读取某条观察保存的旧退件证据；供 I2b3 交还原 owner 时使用。
pub(crate) async fn legacy_return_evidence_for_observation(
    connection: &mut PgConnection,
    observation_id: Uuid,
) -> Result<LegacyReturnEvidence, WriteError> {
    let value: Option<Value> = sqlx::query_scalar(
        "SELECT derived_facts FROM public.mail_envelope_observations WHERE id = $1",
    )
    .bind(observation_id)
    .fetch_optional(&mut *connection)
    .await?;
    value
        .as_ref()
        .map(parse_derived_facts)
        .transpose()?
        .map(|facts| facts.legacy_return_evidence)
        .ok_or_else(|| invariant("consignment_observation_missing"))
}

/// 把数据库清单的逐件身份补充为领域观察；卖家映射由锁内重读传入。
///
/// 逐件 `history_ok` 仍是观察时快照；决定与解除路径必须再经锁内历史重算覆盖。
pub(crate) fn observation_from_stored(
    stored: &StoredObservation,
    seller_user_id: Option<auction_domain::UserId>,
) -> EnvelopeObservation {
    EnvelopeObservation {
        observation_id: stored.observation_id,
        session_binding_digest: stored.session_binding_digest,
        manifest_digest: stored.manifest_digest,
        parser_version: stored.parser_version.clone(),
        subject_is_consign: stored.subject_is_consign,
        sender_identity: stored.sender_identity,
        list_detail_consistent: stored.list_detail_consistent,
        asset_count: stored.asset_count,
        duplicate_raw_eid: stored.duplicate_raw_eid,
        canonical_alias_ambiguous: stored.canonical_alias_ambiguous,
        unknown_attachment: stored.unknown_attachment,
        credits: stored.credits,
        cod: stored.cod,
        returnable: stored.returnable.unwrap_or(false),
        takeable: stored.takeable,
        system_rts: stored.system_rts,
        legacy_auction_owner: stored.legacy_auction_owner,
        items: stored
            .items
            .iter()
            .map(|item| EnvelopeItemObservation { seller_user_id, ..item.clone() })
            .collect(),
    }
}
