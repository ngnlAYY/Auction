//! 投递处置事实的唯一写入口与读取（W2）。
//!
//! 只追加表 `delivery_dispositions` 由本模块独占写入；Admin 对账、装备损失、人工 Sent
//! 证明、自动 Sent 恢复、预检退役与操作者确认都在产生该终态的**同一事务**内经
//! [`append`] 追加处置，不各自维护第二套状态机，也不改写投递状态、资金或资产事实。
//!
//! `resolves_alert` 是写入时按 [`auction_domain::DeliveryDispositionKind::resolves_alert`]
//! 固定的事实列：读侧只按该列过滤“未处置”，不再按来源拼接排除谓词。

use auction_domain::{
    DeliveryDispositionActorSource, DeliveryDispositionKind, DeliveryDispositionSource,
    validate_disposition_actor, validate_disposition_reason, validate_disposition_reason_code,
};
use serde_json::Value;
use sqlx::Postgres;
use uuid::Uuid;

use super::TrackedTransaction as Transaction;

#[cfg(feature = "postgres")]
mod acknowledge;
#[cfg(feature = "postgres")]
mod records;

/// 在产生终态的事务内追加一条处置事实。
pub(crate) struct DeliveryDispositionFact<'a> {
    pub delivery_id: Uuid,
    /// 该处置作用到的投递 fence；读侧按当前 fence 匹配。
    pub fencing_version: i64,
    pub kind: DeliveryDispositionKind,
    pub source: DeliveryDispositionSource,
    pub actor_source: DeliveryDispositionActorSource,
    pub actor_id: &'a str,
    pub reason: Option<&'a str>,
    /// 处置原因码：预检退役为失败码闭集，其它种类为可选稳定标签。
    pub reason_code: Option<&'a str>,
    /// 低基数安全元数据；不含 payload、正文、凭据或远端原文。
    pub detail: Option<&'a Value>,
    /// 操作者命令的幂等身份；只有 `operator_acknowledged` 携带。
    pub command_id: Option<Uuid>,
    pub request_digest: Option<[u8; 32]>,
}

/// 追加处置事实；同一投递、fence 与种类只允许一条，重复写入幂等忽略。
///
/// 准入校验在写入前复用领域规则；校验失败映射为明确的协议错误，调用方事务一并回滚。
pub(crate) async fn append(
    transaction: &mut Transaction<'_, Postgres>,
    fact: DeliveryDispositionFact<'_>,
) -> Result<(), sqlx::Error> {
    validate(&fact)?;
    let (binding_world, binding_mail_id) = match &fact.source.sent_binding {
        Some((world, external_mail_id)) => (Some(world.as_str()), Some(external_mail_id.as_str())),
        None => (None, None),
    };
    let detail = fact.detail.cloned().unwrap_or_else(|| serde_json::json!({}));
    let inserted = sqlx::query(
        r#"INSERT INTO public.delivery_dispositions
               (delivery_id, fencing_version, kind, command_id, request_digest,
                sent_confirmation_id, admin_command_id, equipment_loss_id,
                legacy_self_reconciliation_id, sent_binding_world_id,
                sent_binding_external_mail_id, settlement_recovery_id, source_audit_id,
                actor_source, actor_id, reason, reason_code, resolves_alert, detail)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14,
                   $15, $16, $17, $18, $19)
           ON CONFLICT (delivery_id, kind, fencing_version) DO NOTHING"#,
    )
    .bind(fact.delivery_id)
    .bind(fact.fencing_version)
    .bind(fact.kind.as_str())
    .bind(fact.command_id)
    .bind(fact.request_digest.as_ref().map(|digest| digest.as_slice()))
    .bind(fact.source.sent_confirmation_id)
    .bind(fact.source.admin_command_id)
    .bind(fact.source.equipment_loss_id)
    .bind(fact.source.legacy_self_reconciliation_id)
    .bind(binding_world)
    .bind(binding_mail_id)
    .bind(fact.source.settlement_recovery_id)
    .bind(fact.source.audit_id)
    .bind(fact.actor_source.as_str())
    .bind(fact.actor_id)
    .bind(fact.reason)
    .bind(fact.reason_code)
    .bind(fact.kind.resolves_alert(fact.reason_code))
    .bind(detail)
    .execute(&mut **transaction)
    .await?;
    if inserted.rows_affected() > 1 {
        return Err(protocol_error("投递处置事实写入返回了多条行"));
    }
    Ok(())
}

fn validate(fact: &DeliveryDispositionFact<'_>) -> Result<(), sqlx::Error> {
    if fact.delivery_id.is_nil()
        || fact.fencing_version < 1
        || fact.kind.is_operator_command() != fact.command_id.is_some()
        || fact.command_id.is_some() != fact.request_digest.is_some()
        || fact.command_id.is_some_and(|command_id| command_id.is_nil())
    {
        return Err(protocol_error("投递处置事实的身份不完整"));
    }
    // 与数据库 CHECK 同义的准入：任一条失败即拒绝，绝不写入半成品。
    let admission = fact
        .source
        .validate(fact.kind)
        .and_then(|()| validate_disposition_reason_code(fact.kind, fact.reason_code))
        .and_then(|()| validate_disposition_actor(fact.actor_id))
        .and_then(|()| validate_disposition_reason(fact.kind, fact.reason));
    if let Err(error) = admission {
        let _ = error;
        return Err(protocol_error("投递处置事实未通过领域准入"));
    }
    Ok(())
}

fn protocol_error(message: &str) -> sqlx::Error {
    sqlx::Error::Protocol(message.to_owned())
}

#[cfg(feature = "postgres")]
impl auction_application::delivery::DeliveryDispositionPort for super::DatabaseStore {
    fn dispositions(
        &self,
        delivery_id: Uuid,
    ) -> auction_application::auction::ApplicationFuture<
        '_,
        Vec<auction_application::delivery::DeliveryDispositionRecord>,
    > {
        Box::pin(async move {
            records::dispositions_for_delivery(self.tracked_postgres_pool(), delivery_id).await
        })
    }

    fn acknowledge(
        &self,
        command: auction_application::delivery::DeliveryAcknowledgementCommand,
    ) -> auction_application::auction::ApplicationFuture<
        '_,
        auction_application::delivery::DeliveryAcknowledgementResult,
    > {
        Box::pin(async move {
            acknowledge::acknowledge(self.tracked_postgres_pool(), &command)
                .await
                .map_err(auction_application::core::ApplicationError::from)
        })
    }
}

#[cfg(test)]
mod tests {
    use auction_domain::PREFLIGHT_LOT_WATCH_INVALIDATED;
    use uuid::Uuid;

    use super::{DeliveryDispositionFact, validate};
    use auction_domain::{
        DeliveryDispositionActorSource, DeliveryDispositionKind, DeliveryDispositionSource,
    };

    fn fact(kind: DeliveryDispositionKind) -> DeliveryDispositionFact<'static> {
        DeliveryDispositionFact {
            delivery_id: Uuid::from_u128(1),
            fencing_version: 2,
            kind,
            source: DeliveryDispositionSource::default(),
            actor_source: DeliveryDispositionActorSource::Worker,
            actor_id: "delivery-worker",
            reason: None,
            reason_code: None,
            detail: None,
            command_id: None,
            request_digest: None,
        }
    }

    #[test]
    fn preflight_retirement_needs_only_the_delivery_row_and_failure_code() {
        let mut retired = fact(DeliveryDispositionKind::PreflightRetired);
        retired.reason_code = Some(PREFLIGHT_LOT_WATCH_INVALIDATED);
        assert!(validate(&retired).is_ok());

        let mut unknown_code = fact(DeliveryDispositionKind::PreflightRetired);
        unknown_code.reason_code = Some("mail_disposition_conflict");
        assert!(validate(&unknown_code).is_err());

        let mut with_source = fact(DeliveryDispositionKind::PreflightRetired);
        with_source.reason_code = Some(PREFLIGHT_LOT_WATCH_INVALIDATED);
        with_source.source.audit_id = Some(Uuid::from_u128(9));
        assert!(validate(&with_source).is_err());
    }

    #[test]
    fn operator_acknowledgement_requires_command_identity_and_reason() {
        fn acknowledged() -> DeliveryDispositionFact<'static> {
            let mut fact = fact(DeliveryDispositionKind::OperatorAcknowledged);
            fact.source.audit_id = Some(Uuid::from_u128(3));
            fact.command_id = Some(Uuid::from_u128(4));
            fact.request_digest = Some([7; 32]);
            fact.actor_source = DeliveryDispositionActorSource::Cli;
            fact.actor_id = "operator";
            fact.reason = Some("历史的通知失败已核对，无资金或资产影响");
            fact
        }

        assert!(validate(&acknowledged()).is_ok());

        let mut missing_reason = acknowledged();
        missing_reason.reason = None;
        assert!(validate(&missing_reason).is_err());

        let mut missing_command = acknowledged();
        missing_command.command_id = None;
        missing_command.request_digest = None;
        assert!(validate(&missing_command).is_err());
    }

    #[test]
    fn source_reference_must_match_the_kind() {
        let mut admin = fact(DeliveryDispositionKind::AdminFailed);
        admin.source.admin_command_id = Some(Uuid::from_u128(5));
        admin.actor_source = DeliveryDispositionActorSource::Admin;
        admin.actor_id = "admin";
        assert!(validate(&admin).is_ok());

        let mut mismatched = fact(DeliveryDispositionKind::EquipmentLoss);
        mismatched.source.admin_command_id = Some(Uuid::from_u128(5));
        assert!(validate(&mismatched).is_err());
    }
}
