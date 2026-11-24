//! 整封终局、资产工作与冻结清单的持久化。
//!
//! 终局、唯一工作、投递外壳、逐件事实与回执必须在同一顶层事务提交；本模块只写业务行，
//! 命令行由 4K 的 `DepositWriter::finish` 最后写入。

use auction_application::consignment::DispositionAction;
use auction_application::consignment::{
    AppendQuarantineObservationCommand, ConsignmentError, DepositActor, DepositBusinessRejection,
    DepositOperation, ObservationReceipt,
};
use auction_application::mail::{DispositionManifest, DispositionManifestAttachment};
use auction_domain::consignment::OriginalMailKey;
use auction_domain::consignment::deposit::{EnvelopeDecision, ReturnManifest};
use auction_time::Timestamp;
use serde_json::{Value, json};
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use super::identity::{MailKeyColumns, load_quarantine};
use super::observations::{self, StoredObservation};
use super::{Outcome, conflict, replay_receipt};
use super::{invariant, invariant_decoding};
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::commands::ledger::{DepositArbitration, DepositTarget};
use crate::database::consignment::commands::{LockKeys, with_deposit_transaction};
use crate::database::consignment::deposit::primitives::database_now;
use crate::database::consignment::deposit::repository::DepositRepository;

/// 一条资产工作的创建输入。
pub(crate) struct WorkSeed<'a> {
    /// 工作 UUID。
    pub(crate) work_id: Uuid,
    /// 投递 UUID。
    pub(crate) delivery_id: Uuid,
    /// 投递命令 UUID；与业务命令不同。
    pub(crate) delivery_command_id: Uuid,
    /// 最终决定 UUID。
    pub(crate) decision_id: Uuid,
    /// 资产动作。
    pub(crate) action: DispositionAction,
    /// 原信键。
    pub(crate) mail_key: &'a OriginalMailKey,
    /// 当前观察。
    pub(crate) observation: &'a StoredObservation,
    /// 主题；寄售邮件固定 `consign`，旧 owner 分支不创建工作。
    pub(crate) subject: &'a str,
    /// 拒件清单；Return 时必填。
    pub(crate) return_manifest: Option<&'a ReturnManifest>,
    /// 提交时刻。
    pub(crate) now: Timestamp,
}

/// 处置投递的确定性系统身份；事务重试与命令重放不得换 ID。
pub(crate) fn delivery_identity(decision_id: Uuid, action: DispositionAction) -> (Uuid, Uuid) {
    const NAMESPACE: &str = "auction.consignment.disposition.delivery.v1";
    let business_key = format!("{decision_id}\0{}\0", action.as_str());
    let delivery_id = Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        format!("{NAMESPACE}\0delivery\0{business_key}").as_bytes(),
    );
    let command_id = Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        format!("{NAMESPACE}\0command\0{business_key}").as_bytes(),
    );
    (delivery_id, command_id)
}

/// 写入唯一 Take / Return 工作及其投递外壳。
pub(crate) async fn create_work_in_tx(
    connection: &mut PgConnection,
    seed: &WorkSeed<'_>,
) -> Result<(), WriteError> {
    let columns = MailKeyColumns::from_key(seed.mail_key);
    let action = seed.action.as_str();
    let recipient = seed
        .observation
        .recipient
        .clone()
        .ok_or_else(|| invariant("consignment_work_recipient"))?;
    let manifest = manifest_payload(seed);
    let return_payload = seed.return_manifest.map(return_manifest_payload);
    let payload = json!({
        "schema_version": 2,
        "operation": action,
        "world": columns.world_id,
        "external_mail_id": columns.external_mid,
        "mail_key": {
            "bot_uid": columns.bot_uid,
            "world_id": columns.world_id,
            "external_mid": columns.external_mid,
        },
        "decision_id": seed.decision_id.to_string(),
        "work_id": seed.work_id.to_string(),
        "template": format!("consignment_{action}"),
        "result_code": Value::Null,
        "body": "",
        "user_key_user_id": Value::Null,
        "credits": 0,
        "cod": 0,
        "attachment_ids": [],
        "expected_manifest": manifest,
        "expected_return": return_payload,
    });
    // 投递外壳沿用既有 outbox；寄售来源只由专用载荷与工作行共同授权。
    sqlx::query(
        "INSERT INTO public.deliveries
            (id, command_id, request_digest, origin_kind, origin_id, recipient, payload,
             status, fencing_version, attempt_count, created_at, updated_at)
         VALUES ($1, $2, $3, 'consignment_disposition', $4, $5, $6, 'prepared', 0, 0, $7, $7)",
    )
    .bind(seed.delivery_id)
    .bind(seed.delivery_command_id)
    .bind(seed.observation.manifest_digest.as_slice())
    .bind(format!("consignment:{}:{}", seed.decision_id, seed.work_id))
    .bind(&recipient)
    .bind(&payload)
    .bind(seed.now)
    .execute(&mut *connection)
    .await?;
    sqlx::query(
        "INSERT INTO public.mail_disposition_work
            (id, bot_uid, world_id, external_mid, decision_id, notice_id, action,
             delivery_id, expected_observation_id, manifest_digest, created_at)
         VALUES ($1, $2, $3, $4, $5, NULL, $6, $7, $8, $9, $10)",
    )
    .bind(seed.work_id)
    .bind(columns.bot_uid)
    .bind(columns.world_id)
    .bind(columns.external_mid)
    .bind(seed.decision_id)
    .bind(action)
    .bind(seed.delivery_id)
    .bind(seed.observation.observation_id)
    .bind(seed.observation.manifest_digest.as_slice())
    .bind(seed.now)
    .execute(&mut *connection)
    .await?;
    Ok(())
}

fn manifest_payload(seed: &WorkSeed<'_>) -> Value {
    let attachments = seed
        .observation
        .items
        .iter()
        .filter(|item| {
            item.kind == auction_domain::consignment::deposit::EnvelopeItemKind::Equipment
        })
        .map(|item| {
            json!({
                "ordinal": item.ordinal,
                "name": item.display_name.clone().unwrap_or_default(),
                "canonical_eid": item.canonical_eid,
            })
        })
        .collect::<Vec<_>>();
    json!({
        "world": auction_domain::consignment::settlement_world_code(seed.mail_key.world()),
        "external_mail_id": seed.mail_key.external_mid(),
        "sender": seed.observation.sender.clone().unwrap_or_default(),
        "recipient": seed.observation.recipient.clone().unwrap_or_default(),
        "subject": seed.subject,
        "body_digest": seed.observation.body_digest.to_vec(),
        "credits": seed.observation.credits.unwrap_or(0),
        "cod": seed.observation.cod.unwrap_or(0),
        "received_minute_raw": seed.observation.received_minute_raw,
        "received_at": seed.observation.received_at,
        "attachments": attachments,
        "manifest_digest": seed.observation.manifest_digest.to_vec(),
    })
}

fn return_manifest_payload(manifest: &ReturnManifest) -> Value {
    json!({
        "items": manifest.items.iter().map(|item| json!({
            "ordinal": item.ordinal,
            "canonical_eid": item.canonical_eid,
            "reason": item.reason.as_str(),
        })).collect::<Vec<_>>(),
    })
}

/// 从投递载荷解出冻结清单；摘要以工作行持久值为准。
pub(crate) fn manifest_from_payload(
    payload: &Value,
    manifest_digest: [u8; 32],
) -> Result<DispositionManifest, WriteError> {
    let Some(frozen) = payload.get("expected_manifest") else {
        return Err(invariant("consignment_work_manifest"));
    };
    let world = frozen
        .get("world")
        .and_then(Value::as_str)
        .and_then(auction_domain::consignment::parse_settlement_world)
        .ok_or_else(|| invariant("consignment_work_manifest_world"))?;
    let attachments = frozen
        .get("attachments")
        .and_then(Value::as_array)
        .ok_or_else(|| invariant("consignment_work_manifest_attachments"))?
        .iter()
        .map(|item| {
            Ok(DispositionManifestAttachment {
                ordinal: item
                    .get("ordinal")
                    .and_then(Value::as_u64)
                    .and_then(|value| u32::try_from(value).ok())
                    .ok_or_else(|| invariant("consignment_work_manifest_ordinal"))?,
                name: item
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .ok_or_else(|| invariant("consignment_work_manifest_name"))?,
                canonical_eid: item.get("canonical_eid").and_then(Value::as_str).map(str::to_owned),
            })
        })
        .collect::<Result<Vec<_>, WriteError>>()?;
    let body_digest = digest(frozen.get("body_digest"), "consignment_work_manifest_body_digest")?;
    Ok(DispositionManifest {
        world,
        external_mail_id: text(frozen, "external_mail_id", "consignment_work_manifest_mid")?,
        sender: text(frozen, "sender", "consignment_work_manifest_sender")?,
        recipient: text(frozen, "recipient", "consignment_work_manifest_recipient")?,
        subject: text(frozen, "subject", "consignment_work_manifest_subject")?,
        body_digest,
        credits: integer(frozen, "credits", "consignment_work_manifest_credits")?,
        cod: integer(frozen, "cod", "consignment_work_manifest_cod")?,
        received_minute_raw: text(
            frozen,
            "received_minute_raw",
            "consignment_work_manifest_minute",
        )?,
        received_at: serde_json::from_value(
            frozen
                .get("received_at")
                .cloned()
                .ok_or_else(|| invariant("consignment_work_manifest_time"))?,
        )
        .map_err(invariant_decoding("consignment_work_manifest_time"))?,
        attachments,
        manifest_digest,
    })
}

fn text(value: &Value, field: &'static str, reason: &'static str) -> Result<String, WriteError> {
    value.get(field).and_then(Value::as_str).map(str::to_owned).ok_or_else(|| invariant(reason))
}

fn integer(value: &Value, field: &'static str, reason: &'static str) -> Result<u64, WriteError> {
    value.get(field).and_then(Value::as_u64).ok_or_else(|| invariant(reason))
}

fn digest(value: Option<&Value>, reason: &'static str) -> Result<[u8; 32], WriteError> {
    let bytes = value
        .and_then(Value::as_array)
        .ok_or_else(|| invariant(reason))?
        .iter()
        .map(|byte| byte.as_u64().and_then(|value| u8::try_from(value).ok()))
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| invariant(reason))?;
    bytes.try_into().ok().ok_or_else(|| invariant(reason))
}

/// 写入首次决定；隔离与普通终局都先有这一行。
#[allow(clippy::too_many_arguments, reason = "逐列复刻不可变决定事实")]
pub(crate) async fn insert_initial_decision(
    connection: &mut PgConnection,
    key: &OriginalMailKey,
    decision_id: Uuid,
    observation_id: Uuid,
    command_id: Uuid,
    kind: &str,
    quarantine_id: Option<Uuid>,
    now: Timestamp,
) -> Result<(), WriteError> {
    let columns = MailKeyColumns::from_key(key);
    sqlx::query(
        "INSERT INTO public.mail_initial_decisions
            (bot_uid, world_id, external_mid, id, observation_id, command_id, kind,
             quarantine_id, decided_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
    )
    .bind(columns.bot_uid)
    .bind(columns.world_id)
    .bind(columns.external_mid)
    .bind(decision_id)
    .bind(observation_id)
    .bind(command_id)
    .bind(kind)
    .bind(quarantine_id)
    .bind(now)
    .execute(&mut *connection)
    .await?;
    Ok(())
}

/// 写入最终处置。
pub(crate) async fn insert_final_decision(
    connection: &mut PgConnection,
    key: &OriginalMailKey,
    decision_id: Uuid,
    kind: &str,
    observation_id: Uuid,
    command_id: Uuid,
    now: Timestamp,
) -> Result<(), WriteError> {
    let columns = MailKeyColumns::from_key(key);
    sqlx::query(
        "INSERT INTO public.mail_final_dispositions
            (bot_uid, world_id, external_mid, id, kind, source_observation_id, command_id,
             decided_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(columns.bot_uid)
    .bind(columns.world_id)
    .bind(columns.external_mid)
    .bind(decision_id)
    .bind(kind)
    .bind(observation_id)
    .bind(command_id)
    .bind(now)
    .execute(&mut *connection)
    .await?;
    Ok(())
}

/// 决定对应的终局 kind；隔离不占终局槽。
pub(crate) fn decision_kind(decision: &EnvelopeDecision) -> &'static str {
    match decision {
        EnvelopeDecision::Accept(_) => "accepted",
        EnvelopeDecision::Return(_) => "return_decided",
        EnvelopeDecision::ArchiveEmpty => "archived_empty",
        EnvelopeDecision::Quarantine(_) => "quarantined",
    }
}

/// 非隔离终局必须有工作或显式零工作。
pub(crate) fn work_action(decision: &EnvelopeDecision) -> Option<DispositionAction> {
    match decision {
        EnvelopeDecision::Accept(_) => Some(DispositionAction::Take),
        EnvelopeDecision::Return(_) => Some(DispositionAction::Return),
        EnvelopeDecision::ArchiveEmpty | EnvelopeDecision::Quarantine(_) => None,
    }
}

/// 追加隔离观察；运行时账号只以 INSERT 复制观察，不依赖观察表的 UPDATE 权限。
pub(crate) async fn append_observation_port(
    repository: &DepositRepository,
    command: AppendQuarantineObservationCommand,
) -> Result<ObservationReceipt, ConsignmentError> {
    let pool = repository.store().pool();
    let mut pre =
        pool.acquire().await.map_err(|error| WriteError::from(error).into_application())?;
    let quarantine = load_quarantine(&mut pre, command.quarantine_id)
        .await
        .map_err(WriteError::into_application)?
        .ok_or(ConsignmentError::Deposit(DepositBusinessRejection::ListingNotActive))?;
    drop(pre);
    let key = quarantine.mail_key.clone();
    let sellers: Vec<Uuid> = Vec::new();
    let equipment: Vec<auction_domain::consignment::ListingEquipmentIdentity> = Vec::new();
    let keys = LockKeys {
        command_id: command.command_id.as_uuid(),
        sellers: &sellers,
        equipment: &equipment,
    };
    let mail_keys = [key.clone()];
    let input = command;
    let transaction =
        with_deposit_transaction(repository.store(), keys, &mail_keys, &input, |input, tx| {
            let sellers = sellers.clone();
            let equipment = equipment.clone();
            let key = key.clone();
            Box::pin(async move {
                tx.ensure_lock_set(&sellers, &equipment, std::slice::from_ref(&key))?;
                let arbitration = tx
                    .arbitrate(
                        DepositActor::Worker,
                        DepositOperation::AppendQuarantineObservation,
                        DepositTarget::Quarantine(input.quarantine_id),
                        input.request_hash,
                        None,
                    )
                    .await?;
                let mut writer = match arbitration {
                    DepositArbitration::Replay(stored) => {
                        return match replay_receipt::<ObservationReceipt>(&stored)? {
                            Outcome::Accepted(receipt) => Ok(Outcome::Accepted(receipt)),
                            Outcome::Rejected(_) => {
                                Err(invariant("consignment_observation_replay_rejected"))
                            }
                        };
                    }
                    DepositArbitration::First(writer) => writer,
                };
                let now = database_now(writer.connection()).await?;
                let quarantine =
                    load_quarantine(writer.connection(), input.quarantine_id)
                        .await?
                        .ok_or_else(|| conflict(DepositBusinessRejection::ListingNotActive))?;
                if !quarantine.active {
                    // 裁定：工作重新领取时若隔离已被同一系统命令解除，说明上一轮已成功
                    // 追加观察并完成交接，只是收口工作失败；直接复用该观察完成工作，
                    // 不再转 reconcile（不变量 5、6）。
                    if let Some(receipt) = super::identity::completed_by_same_system_release(
                        writer.connection(),
                        input,
                    )
                    .await?
                    {
                        let _ = writer.finish(receipt.clone(), now).await?;
                        return Ok(Outcome::Accepted(receipt));
                    }
                    return Err(conflict(DepositBusinessRejection::DispositionConflict));
                }
                let leased: bool = sqlx::query_scalar(
                    "SELECT EXISTS (
                        SELECT 1 FROM public.mail_quarantine_rechecks
                        WHERE quarantine_id = $1 AND state = 'leased'
                          AND lease_until > clock_timestamp()
                     )",
                )
                .bind(input.quarantine_id)
                .fetch_one(writer.connection())
                .await?;
                if !leased {
                    return Err(ConsignmentError::LeaseLost.into());
                }
                let linked_remote = super::quarantine::linked_remote_id(
                    input.quarantine_id,
                    input.remote_observation_id.as_bytes(),
                );
                let existing = sqlx::query(
                    "SELECT id, manifest_digest, quarantine_version
                     FROM public.mail_envelope_observations
                     WHERE quarantine_id = $1 AND remote_observation_id = $2",
                )
                .bind(input.quarantine_id)
                .bind(&linked_remote)
                .fetch_optional(writer.connection())
                .await?;
                if let Some(row) = existing {
                    let digest: &[u8] = row.try_get("manifest_digest")?;
                    let digest = <[u8; 32]>::try_from(digest)
                        .map_err(invariant_decoding("consignment_observation_digest"))?;
                    if digest != input.observation.manifest_digest {
                        sqlx::query(
                            "INSERT INTO public.mail_observation_conflicts
                                (observation_id, conflicting_digest, evidence_ref, observed_at)
                             VALUES ($1, $2, $3, $4)
                             ON CONFLICT (observation_id, conflicting_digest) DO NOTHING",
                        )
                        .bind(row.try_get::<Uuid, _>("id")?)
                        .bind(input.observation.manifest_digest.as_slice())
                        .bind(input.evidence_ref.as_deref())
                        .bind(input.observed_at)
                        .execute(writer.connection())
                        .await?;
                        let rejection = DepositBusinessRejection::EvidenceConflict;
                        writer
                            .finish_rejection(
                                crate::database::consignment::commands::DepositRejection::Business(
                                    rejection,
                                ),
                                now,
                            )
                            .await?;
                        return Ok(Outcome::Rejected(rejection));
                    }
                    let first_version = row
                        .try_get::<Option<i64>, _>("quarantine_version")?
                        .and_then(|value| u64::try_from(value).ok())
                        .ok_or_else(|| invariant("consignment_observation_version"))?;
                    let receipt = ObservationReceipt::appended(
                        input.command_id,
                        input.quarantine_id,
                        first_version,
                        row.try_get("id")?,
                        digest,
                    );
                    let _ = writer.finish(receipt.clone(), now).await?;
                    return Ok(Outcome::Accepted(receipt));
                }
                // 同一隔离、同一清单摘要、同一会话绑定摘要视为同一远端观察。旧读取器把
                // 每次领取的 work/token 混进远端身份；这里按语义身份再查一次，避免同一
                // 远端内容因领取次数不同而追加观察、抬高隔离版本（不变量 5、6）。
                if let Some(row) =
                    super::identity::same_observation_by_identity(writer.connection(), input)
                        .await?
                {
                    let digest: &[u8] = row.try_get("manifest_digest")?;
                    let digest = <[u8; 32]>::try_from(digest)
                        .map_err(invariant_decoding("consignment_observation_digest"))?;
                    let version = row
                        .try_get::<Option<i64>, _>("quarantine_version")?
                        .and_then(|value| u64::try_from(value).ok())
                        .ok_or_else(|| invariant("consignment_observation_version"))?;
                    let receipt = ObservationReceipt::appended(
                        input.command_id,
                        input.quarantine_id,
                        version,
                        row.try_get("id")?,
                        digest,
                    );
                    let _ = writer.finish(receipt.clone(), now).await?;
                    return Ok(Outcome::Accepted(receipt));
                }
                let stored = observations::load_observation(
                    writer.connection(),
                    &key,
                    input.observation.observation_id,
                )
                .await?
                .ok_or_else(|| conflict(DepositBusinessRejection::EvidenceUnproven))?;
                if stored.manifest_digest != input.observation.manifest_digest
                    || stored.session_binding_digest != input.observation.session_binding_digest
                {
                    return Err(conflict(DepositBusinessRejection::EvidenceConflict));
                }
                let next_version = quarantine
                    .version
                    .checked_add(1)
                    .ok_or_else(|| invariant("consignment_quarantine_version_overflow"))?;
                let advanced = sqlx::query(
                    "UPDATE public.mail_quarantines
                     SET version = $2, updated_at = $3
                     WHERE id = $1 AND state = 'active' AND version = $4",
                )
                .bind(input.quarantine_id)
                .bind(i64::try_from(next_version)?)
                .bind(now)
                .bind(i64::try_from(quarantine.version)?)
                .execute(writer.connection())
                .await?;
                if advanced.rows_affected() != 1 {
                    return Err(conflict(DepositBusinessRejection::StaleVersion));
                }
                let linked_observation = Uuid::new_v4();
                observations::write_observation_in_tx(
                    writer.connection(),
                    observations::ObservationWrite::Copy {
                        source_id: stored.observation_id,
                        observation_id: linked_observation,
                        remote_observation_id: &linked_remote,
                        quarantine_id: input.quarantine_id,
                        quarantine_version: next_version,
                        legacy_auction_owner: input.observation.legacy_auction_owner,
                    },
                )
                .await?;
                let receipt = ObservationReceipt::appended(
                    input.command_id,
                    input.quarantine_id,
                    next_version,
                    linked_observation,
                    stored.manifest_digest,
                );
                let _ = writer.finish(receipt.clone(), now).await?;
                Ok(Outcome::Accepted(receipt))
            })
        });
    transaction.await.map_err(WriteError::into_application)?.into_result()
}
