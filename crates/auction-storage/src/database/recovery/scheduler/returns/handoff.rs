//! 隔离解除交给原恢复流程时的唯一接管事实：锁原恢复记录，复核系统来源、义务关系、世界与
//! 三条恢复腿，只追加 `settlement_recovery_return_handoffs`。不写消费、不排动作、不改恢复行；
//! 旧恢复消费者在下一事务按原分类消费同一封退件。

use auction_application::settlement::{recovery_return_evidence, system_return_matches_obligation};
use auction_domain::SettlementObligationKind;
use auction_time::Timestamp;
use sqlx::{Postgres, Row};
use uuid::Uuid;

use super::model::RecoveryReturnRow;
use super::validation::expected_recovery_return;
use crate::database::TrackedTransaction as Transaction;
use crate::database::recovery::scheduler::mapping::map_storage_error;
use crate::database::recovery::scheduler::mapping::valid_external_reference;
use crate::database::recovery::scheduler::repository_error;

/// 原恢复流程的持久接管引用。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RecoveryHandoff {
    /// 接管事实 UUID。
    pub(crate) handoff_id: Uuid,
    /// 按原恢复腿确定的义务 UUID。
    pub(crate) obligation_id: Uuid,
    /// 接管事实所属的恢复记录。
    pub(crate) recovery_id: Uuid,
}

/// 在调用方事务内记录一封系统 RTS 退件属于指定恢复记录。
///
/// 不变量 2、4、6：只锁原恢复行并追加唯一接管引用；资产是否匹配留给原恢复分类决定，移交本身
/// 不产生 Take、Return、补寄或资金动作。相容重放返回原接管 UUID。
pub(crate) async fn handoff_recovery_return_in_tx(
    transaction: &mut Transaction<'_, Postgres>,
    return_mail_id: Uuid,
    recovery_id: Uuid,
    obligation_id: Uuid,
    observation_id: Uuid,
    now: Timestamp,
) -> Result<RecoveryHandoff, auction_application::core::RepositoryError> {
    if return_mail_id.is_nil() || recovery_id.is_nil() || obligation_id.is_nil() {
        return Err(repository_error("恢复接管引用包含空 UUID"));
    }
    let observation = sqlx::query(
        "SELECT bot_uid, world_id, external_mid,
                derived_facts ->> 'legacy_auction_owner' AS legacy_auction_owner
         FROM public.mail_envelope_observations
         WHERE id = $1 AND quarantine_id IS NOT NULL",
    )
    .bind(observation_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_storage_error)?
    .ok_or_else(|| repository_error("恢复接管引用的隔离观察不存在"))?;
    let observation_bot_uid: String = observation.try_get("bot_uid").map_err(map_storage_error)?;
    let observation_world: String = observation.try_get("world_id").map_err(map_storage_error)?;
    let observation_mid: String = observation.try_get("external_mid").map_err(map_storage_error)?;
    let legacy_owner: Option<String> =
        observation.try_get("legacy_auction_owner").map_err(map_storage_error)?;

    let recovery = sqlx::query(
        "SELECT id, world_id, equipment_id, payment_obligation_id, equipment_obligation_id,
                return_mail_id, stage, cod_return_count, cod_return_limit, state_version,
                created_at
         FROM public.settlement_recoveries
         WHERE id = $1
         FOR UPDATE",
    )
    .bind(recovery_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_storage_error)?
    .ok_or_else(|| repository_error("恢复接管引用的恢复记录不存在"))?;
    let recovery_world: String = recovery.try_get("world_id").map_err(map_storage_error)?;
    let payment_obligation_id: Uuid =
        recovery.try_get("payment_obligation_id").map_err(map_storage_error)?;
    let equipment_obligation_id: Option<Uuid> =
        recovery.try_get("equipment_obligation_id").map_err(map_storage_error)?;
    let existing_return_mail: Option<Uuid> =
        recovery.try_get("return_mail_id").map_err(map_storage_error)?;
    let stage: String = recovery.try_get("stage").map_err(map_storage_error)?;
    if stage == "completed" {
        return Err(repository_error("恢复记录已终结，不能接管退件"));
    }
    if existing_return_mail.is_some_and(|existing| existing != return_mail_id) {
        return Err(repository_error("恢复记录已绑定另一封退件"));
    }
    // 不变量 6：入参义务必须与观察冻结的旧 owner 对应的腿一致；不一致失败关闭，不得改用另一条
    // 腿，否则上游错误形状会被静默修正。付款与交割义务分别有唯一约束，主键列即属于本记录。
    let expected_obligation_id = match legacy_owner.as_deref() {
        Some("winner_equipment_cod" | "winner_payment_carrier_cod") => payment_obligation_id,
        Some("winner_equipment_handoff") => {
            equipment_obligation_id.ok_or_else(|| repository_error("恢复接管缺少交割义务"))?
        }
        _ => return Err(repository_error("恢复接管缺少明确的原恢复腿")),
    };
    if obligation_id != expected_obligation_id {
        return Err(repository_error("恢复接管义务与原恢复腿不一致"));
    }

    let inbound = sqlx::query(
        "SELECT world_id, external_mail_id, mail_kind, status, decision_metadata, received_at,
                sender, subject
         FROM public.inbound_mail WHERE id = $1",
    )
    .bind(return_mail_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_storage_error)?
    .ok_or_else(|| repository_error("恢复接管的入站邮件不存在"))?;
    let return_world: String = inbound.try_get("world_id").map_err(map_storage_error)?;
    let external_mail_id: String =
        inbound.try_get("external_mail_id").map_err(map_storage_error)?;
    let mail_kind: String = inbound.try_get("mail_kind").map_err(map_storage_error)?;
    let status: String = inbound.try_get("status").map_err(map_storage_error)?;
    if mail_kind != "delivery"
        || status != "accepted"
        || !valid_external_reference(&external_mail_id)
        || return_world != observation_world
        || external_mail_id != observation_mid
    {
        return Err(repository_error("恢复接管邮件与隔离观察不同源"));
    }
    // 与结算退件案件互斥：已有案件证据的退件只能留在原案件流程，恢复不得再次接管。
    let case_consumed: bool = sqlx::query_scalar(
        "SELECT EXISTS (
             SELECT 1 FROM public.settlement_return_evidence WHERE return_mail_id = $1
         )",
    )
    .bind(return_mail_id)
    .fetch_one(&mut **transaction)
    .await
    .map_err(map_storage_error)?;
    if case_consumed {
        return Err(repository_error("恢复接管的退件已由结算案件消费"));
    }
    let linked_bot_uid: Option<String> =
        sqlx::query_scalar("SELECT bot_uid FROM public.mail_original_links WHERE inbound_id = $1")
            .bind(return_mail_id)
            .fetch_optional(&mut **transaction)
            .await
            .map_err(map_storage_error)?;
    if linked_bot_uid.is_some_and(|bot_uid| bot_uid != observation_bot_uid) {
        return Err(repository_error("恢复接管邮件的认证 Bot 与原信键不一致"));
    }

    let obligation = sqlx::query(
        "SELECT obligation_kind, carrier_credit, equipment_id
         FROM public.settlement_obligations WHERE id = $1",
    )
    .bind(expected_obligation_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_storage_error)?
    .ok_or_else(|| repository_error("恢复接管的结算义务不存在"))?;
    let obligation_kind: String =
        obligation.try_get("obligation_kind").map_err(map_storage_error)?;
    let carrier_credit: i64 = obligation.try_get("carrier_credit").map_err(map_storage_error)?;
    let obligation_equipment_id: Option<Uuid> =
        obligation.try_get("equipment_id").map_err(map_storage_error)?;
    let equipment_id: Uuid = recovery.try_get("equipment_id").map_err(map_storage_error)?;
    let external_equipment_id: String =
        sqlx::query_scalar("SELECT external_equipment_id FROM public.equipment WHERE id = $1")
            .bind(equipment_id)
            .fetch_one(&mut **transaction)
            .await
            .map_err(map_storage_error)?;
    let return_metadata: serde_json::Value =
        inbound.try_get("decision_metadata").map_err(map_storage_error)?;
    let evidence = recovery_return_evidence(&return_metadata)
        .ok_or_else(|| repository_error("恢复接管邮件的证据对象无效"))?;
    if evidence.obligation_id != expected_obligation_id {
        return Err(repository_error("恢复接管的证据义务与请求不一致"));
    }
    let kind = SettlementObligationKind::from_name(&obligation_kind)
        .ok_or_else(|| repository_error("恢复接管的义务种类无效"))?;
    let sender: Option<String> = inbound.try_get("sender").map_err(map_storage_error)?;
    let subject: Option<String> = inbound.try_get("subject").map_err(map_storage_error)?;
    let source_matches =
        sender.as_deref().zip(subject.as_deref()).is_some_and(|(sender, subject)| {
            system_return_matches_obligation(sender, subject, kind, expected_obligation_id)
        });
    if !source_matches {
        return Err(repository_error("恢复接管邮件不是规范系统 RTS"));
    }

    let row = RecoveryReturnRow {
        recovery_id,
        return_mail_id,
        return_external_mail_id: external_mail_id,
        return_world_id: return_world,
        return_metadata,
        return_received_at: inbound.try_get("received_at").map_err(map_storage_error)?,
        return_sender: sender,
        return_subject: subject,
        returned_obligation_id: expected_obligation_id,
        returned_obligation_kind: obligation_kind,
        returned_carrier_credit: carrier_credit,
        external_equipment_id,
        recovery_world_id: recovery_world,
        cod_return_count: recovery.try_get("cod_return_count").map_err(map_storage_error)?,
        cod_return_limit: recovery.try_get("cod_return_limit").map_err(map_storage_error)?,
        payment_obligation_id,
        equipment_obligation_id,
        state_version: recovery.try_get("state_version").map_err(map_storage_error)?,
        created_at: recovery.try_get("created_at").map_err(map_storage_error)?,
    };
    let expected = expected_recovery_return(&row)
        .ok_or_else(|| repository_error("恢复接管不符合原恢复记录的付款或交割腿"))?;
    if matches!(expected.kind, "reclaim_equipment_take" | "recover_equipment_take")
        && obligation_equipment_id != Some(equipment_id)
    {
        return Err(repository_error("恢复接管的装备义务与恢复记录不一致"));
    }

    if let Some(existing) = sqlx::query(
        "SELECT id, bot_uid, world_id, external_mid, recovery_id, obligation_id
         FROM public.settlement_recovery_return_handoffs
         WHERE return_mail_id = $1",
    )
    .bind(return_mail_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_storage_error)?
    {
        let existing_bot_uid: String = existing.try_get("bot_uid").map_err(map_storage_error)?;
        let existing_world: String = existing.try_get("world_id").map_err(map_storage_error)?;
        let existing_mid: String = existing.try_get("external_mid").map_err(map_storage_error)?;
        let existing_recovery: Uuid = existing.try_get("recovery_id").map_err(map_storage_error)?;
        let existing_obligation: Uuid =
            existing.try_get("obligation_id").map_err(map_storage_error)?;
        if existing_bot_uid != observation_bot_uid
            || existing_world != observation_world
            || existing_mid != observation_mid
            || existing_recovery != recovery_id
            || existing_obligation != expected_obligation_id
        {
            return Err(repository_error("恢复接管重放与首次事实冲突"));
        }
        return Ok(RecoveryHandoff {
            handoff_id: existing.try_get("id").map_err(map_storage_error)?,
            obligation_id: expected_obligation_id,
            recovery_id,
        });
    }
    let key_taken: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM public.settlement_recovery_return_handoffs
         WHERE bot_uid = $1 AND world_id = $2 AND external_mid = $3",
    )
    .bind(&observation_bot_uid)
    .bind(&observation_world)
    .bind(&observation_mid)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_storage_error)?;
    if key_taken.is_some() {
        return Err(repository_error("同一原信键已接管另一封退件"));
    }

    let handoff_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO public.settlement_recovery_return_handoffs
            (id, bot_uid, world_id, external_mid, return_mail_id, recovery_id, obligation_id,
             observation_id, accepted_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
    )
    .bind(handoff_id)
    .bind(&observation_bot_uid)
    .bind(&observation_world)
    .bind(&observation_mid)
    .bind(return_mail_id)
    .bind(recovery_id)
    .bind(expected_obligation_id)
    .bind(observation_id)
    .bind(now)
    .execute(&mut **transaction)
    .await
    .map_err(map_storage_error)?;
    Ok(RecoveryHandoff { handoff_id, obligation_id: expected_obligation_id, recovery_id })
}
