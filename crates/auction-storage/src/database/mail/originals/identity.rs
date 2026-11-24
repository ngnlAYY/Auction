//! 稳定原信键 K 的 SQL 身份与安全文本校验。

use auction_application::consignment::{
    AppendQuarantineObservationCommand, DepositOperation, ObservationReceipt, system_command_id,
};
use auction_domain::SettlementWorld;
use auction_domain::consignment::OriginalMailKey;
use auction_domain::consignment::deposit::{QuarantineReason, ReleaseKind};
use auction_time::Timestamp;
use serde_json::{Value, json};
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use crate::database::consignment::commands::errors::WriteError;

/// 按隔离、清单摘要、会话绑定摘要与旧 owner 派生事实查找已存在的观察行。
pub(crate) async fn same_observation_by_identity(
    connection: &mut PgConnection,
    input: &AppendQuarantineObservationCommand,
) -> Result<Option<sqlx::postgres::PgRow>, WriteError> {
    // 旧证据派生事实也参与身份：首次隔离行没有 owner，重检补出 owner 时必须追加一行
    // 保存该事实，否则自动交接读不到来源；之后同 owner 的重读复用该行。
    let legacy_owner = input
        .observation
        .legacy_auction_owner
        .map(auction_domain::consignment::LegacyAuctionOwner::as_str);
    Ok(sqlx::query(
        "SELECT o.id, o.manifest_digest, o.quarantine_version
         FROM public.mail_envelope_observations AS o
         JOIN public.mail_evidence_sessions AS session ON session.id = o.session_id
         WHERE o.quarantine_id = $1
           AND o.manifest_digest = $2
           AND session.binding_digest = $3
           AND o.derived_facts->>'legacy_auction_owner' IS NOT DISTINCT FROM $4
         ORDER BY o.quarantine_version, o.id
         LIMIT 1",
    )
    .bind(input.quarantine_id)
    .bind(input.observation.manifest_digest.as_slice())
    .bind(input.observation.session_binding_digest.as_slice())
    .bind(legacy_owner)
    .fetch_optional(&mut *connection)
    .await?)
}

/// 隔离已由“隔离 + 本观察”派生的同一系统命令解除时，返回该观察的回执。
pub(crate) async fn completed_by_same_system_release(
    connection: &mut PgConnection,
    input: &AppendQuarantineObservationCommand,
) -> Result<Option<ObservationReceipt>, WriteError> {
    let observation_id = input.observation.observation_id;
    if observation_id.is_nil() {
        return Ok(None);
    }
    let command_id = system_command_id(DepositOperation::ReleaseQuarantine, &{
        let mut key = input.quarantine_id.as_bytes().to_vec();
        key.extend_from_slice(observation_id.as_bytes());
        key
    });
    let row = sqlx::query(
        "SELECT o.id, o.manifest_digest, o.quarantine_version
         FROM public.mail_quarantine_releases AS release
         JOIN public.mail_envelope_observations AS o
           ON o.id = release.observation_id AND o.quarantine_id = release.quarantine_id
         WHERE release.quarantine_id = $1 AND release.command_id = $2
           AND release.observation_id = $3
           AND release.kind IN ('auction_return_case', 'auction_recovery_return')",
    )
    .bind(input.quarantine_id)
    .bind(command_id.as_uuid())
    .bind(observation_id)
    .fetch_optional(&mut *connection)
    .await?;
    let Some(row) = row else { return Ok(None) };
    let digest: &[u8] = row.try_get("manifest_digest")?;
    let digest = <[u8; 32]>::try_from(digest)
        .map_err(super::invariant_decoding("consignment_observation_digest"))?;
    let version = row
        .try_get::<Option<i64>, _>("quarantine_version")?
        .and_then(|value| u64::try_from(value).ok())
        .ok_or_else(|| super::invariant("consignment_observation_version"))?;
    Ok(Some(ObservationReceipt::appended(
        input.command_id,
        input.quarantine_id,
        version,
        row.try_get("id")?,
        digest,
    )))
}

/// 原信键的数据库列投影。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MailKeyColumns<'a> {
    /// 认证 Bot UID 文本。
    pub(crate) bot_uid: &'a str,
    /// 世界码。
    pub(crate) world_id: &'a str,
    /// 规范正十进制 MID。
    pub(crate) external_mid: &'a str,
}

impl<'a> MailKeyColumns<'a> {
    /// 从领域键构造；世界码只接受 main / isekai。
    pub(crate) fn from_key(key: &'a OriginalMailKey) -> Self {
        Self {
            bot_uid: key.authenticated_bot_uid(),
            world_id: auction_domain::consignment::settlement_world_code(key.world()),
            external_mid: key.external_mid(),
        }
    }
}

/// 解析世界码；未知值失败关闭。
pub(crate) fn parse_world(value: &str) -> Result<SettlementWorld, WriteError> {
    auction_domain::consignment::parse_settlement_world(value)
        .ok_or_else(|| super::invariant("consignment_mail_world"))
}

/// 隔离重读定位。存入 `mail_quarantines.reread_locator`，只描述到哪里重读这封信。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct QuarantineMetadata {
    /// 邮箱键。
    pub(crate) mailbox_key: String,
}

impl QuarantineMetadata {
    /// 从邮箱键构造重读定位。
    pub(crate) fn new(mailbox_key: &str) -> Self {
        Self { mailbox_key: mailbox_key.to_owned() }
    }

    /// 编码为受控单行文本；只含定位，不含主题、发件人、owner 或逐件事实。
    pub(crate) fn encode(&self) -> Result<String, WriteError> {
        if self.mailbox_key.is_empty()
            || self.mailbox_key.len() > 256
            || self.mailbox_key.chars().any(char::is_control)
        {
            return Err(super::invariant("consignment_quarantine_locator_size"));
        }
        let value = json!({"v": 2, "b": self.mailbox_key});
        let text = value.to_string();
        if text.len() > 512 {
            return Err(super::invariant("consignment_quarantine_locator_size"));
        }
        Ok(text)
    }

    /// 解码；未知版本或损坏定位失败关闭。旧 v1 只取邮箱键，其他事实不再读取。
    pub(crate) fn decode(text: &str) -> Result<Self, WriteError> {
        let value: Value = serde_json::from_str(text)
            .map_err(super::invariant_decoding("consignment_quarantine_locator_json"))?;
        let version = value.get("v").and_then(Value::as_u64);
        if !matches!(version, Some(1 | 2)) {
            return Err(super::invariant("consignment_quarantine_locator_version"));
        }
        let mailbox_key = text_field(&value, "b")?;
        if mailbox_key.is_empty()
            || mailbox_key.len() > 256
            || mailbox_key.chars().any(char::is_control)
        {
            return Err(super::invariant("consignment_quarantine_locator_size"));
        }
        Ok(Self { mailbox_key })
    }
}

fn text_field(value: &Value, field: &'static str) -> Result<String, WriteError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| super::invariant("consignment_quarantine_locator_field"))
}

/// 隔离事实读取结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StoredQuarantine {
    /// 隔离 UUID。
    pub(crate) quarantine_id: Uuid,
    /// 原信键。
    pub(crate) mail_key: OriginalMailKey,
    /// 当前版本。
    pub(crate) version: u64,
    /// 是否活动。
    pub(crate) active: bool,
    /// 首次决定 UUID。
    pub(crate) initial_decision_id: Uuid,
    /// 首次观察 UUID。
    pub(crate) initial_observation_id: Uuid,
    /// 原因。
    pub(crate) reason: QuarantineReason,
    /// 元数据。
    pub(crate) metadata: QuarantineMetadata,
}

/// 读取隔离事实；未知原因或损坏元数据失败关闭。
pub(crate) async fn load_quarantine(
    connection: &mut PgConnection,
    quarantine_id: Uuid,
) -> Result<Option<StoredQuarantine>, WriteError> {
    let row = sqlx::query(
        "SELECT q.id, q.bot_uid, q.world_id, q.external_mid, q.version, q.state,
                q.initial_decision_id, q.reread_locator, q.responsibility_code,
                initial.observation_id
         FROM public.mail_quarantines AS q
         JOIN public.mail_initial_decisions AS initial
           ON initial.bot_uid = q.bot_uid AND initial.world_id = q.world_id
          AND initial.external_mid = q.external_mid AND initial.id = q.initial_decision_id
         WHERE q.id = $1",
    )
    .bind(quarantine_id)
    .fetch_optional(connection)
    .await?;
    let Some(row) = row else { return Ok(None) };
    let world: String = row.try_get("world_id")?;
    let bot_uid: String = row.try_get("bot_uid")?;
    let external_mid: String = row.try_get("external_mid")?;
    let mail_key = OriginalMailKey::new(&bot_uid, parse_world(&world)?, &external_mid)
        .map_err(super::invariant_decoding("consignment_quarantine_mail_key"))?;
    let reason_text: String = row.try_get("responsibility_code")?;
    let reason = QUARANTINE_REASONS
        .into_iter()
        .find(|reason| reason.as_str() == reason_text)
        .ok_or_else(|| super::invariant("consignment_quarantine_reason"))?;
    let locator: String = row.try_get("reread_locator")?;
    Ok(Some(StoredQuarantine {
        quarantine_id,
        mail_key,
        version: u64::try_from(row.try_get::<i64, _>("version")?)
            .map_err(super::invariant_decoding("consignment_quarantine_version"))?,
        active: row.try_get::<String, _>("state")? == "active",
        initial_decision_id: row.try_get("initial_decision_id")?,
        initial_observation_id: row.try_get("observation_id")?,
        reason,
        metadata: QuarantineMetadata::decode(&locator)?,
    }))
}

const QUARANTINE_REASONS: [QuarantineReason; 12] = [
    QuarantineReason::DuplicateRawEid,
    QuarantineReason::CanonicalAliasAmbiguous,
    QuarantineReason::UnknownAttachment,
    QuarantineReason::ListDetailConflict,
    QuarantineReason::AssetCountUnknown,
    QuarantineReason::SameNameChangedUid,
    QuarantineReason::IdentityUnproven,
    QuarantineReason::AmountUnproven,
    QuarantineReason::ReturnUnproven,
    QuarantineReason::TakeUnproven,
    QuarantineReason::UnsafeReturnShape,
    QuarantineReason::SuspectedSystemRts,
];

/// 把持久责任码解析回领域闭集。
pub(crate) fn quarantine_reason(value: &str) -> Option<QuarantineReason> {
    QUARANTINE_REASONS.into_iter().find(|reason| reason.as_str() == value)
}

/// 读取世界收件能力；没有有效就绪行时不授权。
pub(crate) async fn readiness_capabilities(
    connection: &mut PgConnection,
    world: SettlementWorld,
    now: Timestamp,
) -> Result<auction_domain::consignment::deposit::EnvelopeCapabilities, WriteError> {
    let row = sqlx::query(
        "SELECT take_ready, return_ready
         FROM public.consignment_mail_readiness
         WHERE world_id = $1 AND valid_until > $2",
    )
    .bind(auction_domain::consignment::settlement_world_code(world))
    .bind(now)
    .fetch_optional(connection)
    .await?;
    Ok(match row {
        Some(row) => auction_domain::consignment::deposit::EnvelopeCapabilities {
            safe_original_return: row.try_get("return_ready")?,
            take_ready: row.try_get("take_ready")?,
        },
        None => auction_domain::consignment::deposit::EnvelopeCapabilities {
            safe_original_return: false,
            take_ready: false,
        },
    })
}

/// 已解除事实的安全投影。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StoredRelease {
    /// 解除 UUID。
    pub(crate) release_id: Uuid,
    /// 最终决定 UUID。
    pub(crate) decision_id: Uuid,
    /// 结果种类。
    pub(crate) kind: ReleaseKind,
    /// 原退件邮件。
    pub(crate) return_mail_id: Option<Uuid>,
    /// 原退件证据。
    pub(crate) return_evidence_id: Option<Uuid>,
    /// 恢复记录。
    pub(crate) recovery_id: Option<Uuid>,
    /// 恢复接管事实。
    pub(crate) recovery_handoff_id: Option<Uuid>,
    /// 原义务。
    pub(crate) obligation_id: Option<Uuid>,
}

/// 按 K 读取已解除事实。
pub(crate) async fn load_release(
    connection: &mut PgConnection,
    key: &OriginalMailKey,
) -> Result<Option<StoredRelease>, WriteError> {
    let columns = MailKeyColumns::from_key(key);
    let row = sqlx::query(
        "SELECT r.id, r.decision_id, r.kind, r.case_return_mail_id, r.case_evidence_id,
                r.recovery_handoff_id, h.recovery_id, h.obligation_id,
                h.return_mail_id AS recovery_return_mail_id,
                evidence.obligation_id AS case_obligation_id
         FROM public.mail_quarantine_releases AS r
         LEFT JOIN public.settlement_recovery_return_handoffs AS h
           ON h.id = r.recovery_handoff_id
         LEFT JOIN public.settlement_return_evidence AS evidence
           ON evidence.return_mail_id = r.case_return_mail_id
         WHERE r.bot_uid = $1 AND r.world_id = $2 AND r.external_mid = $3",
    )
    .bind(columns.bot_uid)
    .bind(columns.world_id)
    .bind(columns.external_mid)
    .fetch_optional(connection)
    .await?;
    row.map(|row| {
        let kind: String = row.try_get("kind")?;
        let kind = release_kind(&kind)?;
        let case_return_mail_id: Option<Uuid> = row.try_get("case_return_mail_id")?;
        let recovery_return_mail_id: Option<Uuid> = row.try_get("recovery_return_mail_id")?;
        // E2：案件解除的义务来自原退件证据，可为空；恢复解除仍取接管事实的义务。
        let obligation_id = match kind {
            ReleaseKind::AuctionReturnCase => row.try_get("case_obligation_id")?,
            ReleaseKind::AuctionRecoveryReturn => row.try_get("obligation_id")?,
            _ => None,
        };
        Ok(StoredRelease {
            release_id: row.try_get("id")?,
            decision_id: row.try_get("decision_id")?,
            kind,
            return_mail_id: case_return_mail_id.or(recovery_return_mail_id),
            return_evidence_id: row.try_get("case_evidence_id")?,
            recovery_id: row.try_get("recovery_id")?,
            recovery_handoff_id: row.try_get("recovery_handoff_id")?,
            obligation_id,
        })
    })
    .transpose()
}

fn release_kind(value: &str) -> Result<ReleaseKind, WriteError> {
    match value {
        "accepted" => Ok(ReleaseKind::Accept),
        "return_decided" => Ok(ReleaseKind::Return),
        "archived_empty" => Ok(ReleaseKind::ArchiveEmpty),
        "auction_return_case" => Ok(ReleaseKind::AuctionReturnCase),
        "auction_recovery_return" => Ok(ReleaseKind::AuctionRecoveryReturn),
        "acquire_return" => Ok(ReleaseKind::AcquireReturn),
        _ => Err(super::invariant("consignment_release_kind")),
    }
}

#[cfg(feature = "test-db")]
impl crate::database::consignment::deposit::repository::DepositRepository {
    /// PG 测试入口：直接调用恢复接管原语；成功返回接管 UUID，失败保留具体拒绝分类文本。
    pub async fn test_handoff_recovery_return(
        &self,
        return_mail_id: Uuid,
        recovery_id: Uuid,
        obligation_id: Uuid,
        observation_id: Uuid,
    ) -> Result<Uuid, String> {
        use sqlx::Connection as _;

        let mut connection =
            self.store().pool().acquire().await.map_err(|error| format!("{error:?}"))?;
        let transaction = connection.begin().await.map_err(|error| format!("{error:?}"))?;
        let mut tracked = crate::database::TrackedTransaction::borrowed(transaction);
        match crate::database::recovery::scheduler::handoff_recovery_return_in_tx(
            &mut tracked,
            return_mail_id,
            recovery_id,
            obligation_id,
            observation_id,
            Timestamp::now(),
        )
        .await
        {
            Ok(handoff) => {
                tracked.commit().await.map_err(|error| format!("{error:?}"))?;
                Ok(handoff.handoff_id)
            }
            Err(error) => {
                tracked.rollback().await.map_err(|error| format!("{error:?}"))?;
                Err(error.to_string())
            }
        }
    }
}
