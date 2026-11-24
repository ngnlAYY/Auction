//! 寄售来信在 poll 主循环中的预读与同事务提交。
//!
//! 不变量 1、2、3、5、8：首次观察、整封决定、工作、回执、待读前缀与游标在同一个
//! Serializable 事务中写入；已有决定只推进前缀，不重写首次证据；前缀 CAS 失败整体回滚。

use std::collections::BTreeSet;
use std::fmt::Display;

use auction_application::consignment::{
    ConsignmentError, DepositActor, DepositOperation, EnvelopePreflight, EnvelopeReceipt,
    MailReadinessGate, PollEnvelopeStore, PollEnvelopeWrite,
};
use auction_application::core::ApplicationFuture;
use auction_domain::SettlementWorld;
use auction_domain::UserId;
use auction_domain::consignment::OriginalMailKey;
use auction_domain::consignment::deposit::{
    EnvelopeCapabilities, EnvelopeObservation, ReservationBinding,
};
use auction_time::Timestamp;
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use crate::database::DatabaseStore;
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::commands::ledger::{
    DepositArbitration, DepositTarget, StoredReceipt,
};
use crate::database::consignment::commands::session::DepositTransaction;
use crate::database::consignment::commands::{LockKeys, with_deposit_transaction};
use crate::database::consignment::deposit::primitives::database_now;
use crate::database::mail::originals::identity::MailKeyColumns;
use crate::database::mail::originals::observations;
use crate::database::mail::originals::quarantine::{
    decide_envelope_in_tx, envelope_equipment, envelope_sellers, reservation_binding,
    seller_by_name,
};
use crate::database::mail::originals::{Outcome, replay_receipt};
use crate::database::mail::poll::complete_prefix;

/// 当前轮询租约与前缀的冻结身份。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PrefixClaim<'a> {
    /// 邮箱键。
    pub(crate) mailbox_key: &'a str,
    /// 领取 token。
    pub(crate) claim_token: Uuid,
    /// 领取 owner。
    pub(crate) claim_owner: &'a str,
    /// 领取时的状态版本。
    pub(crate) state_version: i64,
    /// 领取时的游标；必须与锁内现值相等。
    pub(crate) expected_cursor: Option<&'a str>,
    /// 本次要推进到的最后完成项。
    pub(crate) next_cursor: &'a str,
    /// 本次轮询时刻。
    pub(crate) polled_at: Timestamp,
    /// 下一次调度时刻。
    pub(crate) next_poll_at: Timestamp,
}

/// 前缀 CAS 的失败闭集。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PrefixCasError {
    /// 租约、owner 或版本已失效。
    LeaseLost,
    /// 锁内游标已不是领取时的值。
    StalePrefix,
    /// 新游标为空或没有前进。
    NonAdvancing,
}

impl PrefixCasError {
    /// 映射到应用安全错误；前缀竞争需要重新领取，不能伪造成功。
    pub(crate) fn into_application(self) -> ConsignmentError {
        match self {
            Self::LeaseLost => ConsignmentError::LeaseLost,
            Self::StalePrefix | Self::NonAdvancing => ConsignmentError::Unavailable,
        }
    }
}

fn cas_decode<E: Display>(error: E) -> PrefixCasError {
    tracing::warn!(error = %error, "MoogleMail 前缀 CAS 读取失败");
    PrefixCasError::StalePrefix
}

/// 预读失败一律失败关闭；日志只保留数据库安全错误，不输出邮件内容。
fn preflight_unavailable<E: Display>(error: E) -> ConsignmentError {
    tracing::warn!(error = %error, "寄售 poll 预读失败");
    ConsignmentError::Unavailable
}

/// 待读前缀删除失败；调用方必须整体回滚，由下一轮重新领取。
fn prefix_unavailable<E: Display>(error: E) -> WriteError {
    tracing::warn!(error = %error, "MoogleMail 待读前缀删除失败");
    ConsignmentError::Unavailable.into()
}

/// 在同一事务内推进 Inbox 前缀并释放本轮租约。
///
/// 不变量 3、5：只有租约、版本与锁内游标全部匹配时才写入；游标不倒退。
pub(crate) async fn advance_prefix_in_tx(
    connection: &mut PgConnection,
    claim: &PrefixClaim<'_>,
) -> Result<(), PrefixCasError> {
    if claim.next_cursor.is_empty() || claim.next_cursor != claim.next_cursor.trim() {
        return Err(PrefixCasError::NonAdvancing);
    }
    if claim.expected_cursor.is_some_and(|value| value == claim.next_cursor) {
        return Err(PrefixCasError::NonAdvancing);
    }
    let updated = sqlx::query(
        "WITH sql_clock AS MATERIALIZED (SELECT clock_timestamp() AS at)
         UPDATE public.mail_poll_state
         SET cursor_value = $5,
             last_poll_at = GREATEST(last_poll_at, $6),
             next_poll_at = GREATEST(GREATEST(last_poll_at, $6), $7),
             consecutive_failures = 0,
             claim_token = NULL,
             claim_owner = NULL,
             claim_expires_at = NULL,
             last_failure_code = NULL,
             last_failure_detail = NULL,
             last_failure_at = NULL,
             state_version = state_version + 1,
             updated_at = $6
         WHERE mailbox_key = $1
           AND claim_token = $2
           AND claim_owner = $3
           AND state_version = $4
           AND claim_expires_at > (SELECT at FROM sql_clock)
           AND cursor_value IS NOT DISTINCT FROM $8
           AND (
               $8 IS NULL
               OR (
                   $5 COLLATE \"C\" ~ '^[1-9][0-9]*$'
                   AND $8 COLLATE \"C\" ~ '^[1-9][0-9]*$'
                   AND $5::numeric > $8::numeric
               )
               OR (
                   NOT ($5 COLLATE \"C\" ~ '^[1-9][0-9]*$' AND $8 COLLATE \"C\" ~ '^[1-9][0-9]*$')
                   AND $5 COLLATE \"C\" > $8 COLLATE \"C\"
               )
           )
         RETURNING state_version",
    )
    .bind(claim.mailbox_key)
    .bind(claim.claim_token)
    .bind(claim.claim_owner)
    .bind(claim.state_version)
    .bind(claim.next_cursor)
    .bind(claim.polled_at)
    .bind(claim.next_poll_at)
    .bind(claim.expected_cursor)
    .fetch_optional(&mut *connection)
    .await
    .map_err(cas_decode)?;
    if updated.is_some() {
        return Ok(());
    }
    let row = sqlx::query(
        "SELECT cursor_value, claim_token, claim_owner, state_version,
                COALESCE(claim_expires_at > clock_timestamp(), false) AS lease_active
         FROM public.mail_poll_state WHERE mailbox_key = $1",
    )
    .bind(claim.mailbox_key)
    .fetch_optional(&mut *connection)
    .await
    .map_err(cas_decode)?;
    let Some(row) = row else {
        return Err(PrefixCasError::LeaseLost);
    };
    let cursor: Option<String> = sqlx::Row::try_get(&row, "cursor_value").map_err(cas_decode)?;
    let token: Option<Uuid> = sqlx::Row::try_get(&row, "claim_token").map_err(cas_decode)?;
    let owner: Option<String> = sqlx::Row::try_get(&row, "claim_owner").map_err(cas_decode)?;
    let version: i64 = sqlx::Row::try_get(&row, "state_version").map_err(cas_decode)?;
    let lease_active: bool = sqlx::Row::try_get(&row, "lease_active").map_err(cas_decode)?;
    if token != Some(claim.claim_token)
        || owner.as_deref() != Some(claim.claim_owner)
        || version != claim.state_version
        || !lease_active
    {
        return Err(PrefixCasError::LeaseLost);
    }
    if cursor.as_deref() != claim.expected_cursor {
        return Err(PrefixCasError::StalePrefix);
    }
    Err(PrefixCasError::NonAdvancing)
}

impl MailReadinessGate for DatabaseStore {
    fn consignment_candidates_enabled(
        &self,
        world: SettlementWorld,
    ) -> ApplicationFuture<'_, bool> {
        Box::pin(async move {
            let ready: bool = sqlx::query_scalar(
                "SELECT EXISTS (
                    SELECT 1
                    FROM public.consignment_mail_readiness AS readiness
                    JOIN public.mail_evidence_sessions AS session
                      ON session.id = readiness.session_id
                     AND session.world_id = readiness.world_id
                    WHERE readiness.world_id = $1
                      AND readiness.detail_ready = true
                      AND readiness.valid_until > clock_timestamp()
                 )",
            )
            .bind(auction_domain::consignment::settlement_world_code(world))
            .fetch_one(self.pool())
            .await
            .map_err(|error| crate::database::storage_error_from(error, "读取寄售收件门控失败"))?;
            Ok(ready)
        })
    }
}

/// 预读与锁内重读共用的原信绑定事实。
///
/// 两处必须走同一条查询，避免锁外预读与锁内复算的卖家、active 预约集合漂移；锁内结果只用于
/// 与命令事实比较，任何变化都退出本事务重新预读（D9 §1.3）。
struct EnvelopeBinding {
    seller_user_id: Option<UserId>,
    reservations: Vec<ReservationBinding>,
    season_window_id: Option<Uuid>,
}

/// 观察里出现过的规范 EID 集合；两处读取同一集合以保持预约查询一致。
fn canonical_eids(observation: &EnvelopeObservation) -> BTreeSet<String> {
    observation.items.iter().filter_map(|item| item.canonical_eid.clone()).collect()
}

/// 按发件人读取唯一卖家，并读取该卖家在本世界、本封 EID 集合上的 active 预约。
async fn read_envelope_binding(
    connection: &mut PgConnection,
    world: SettlementWorld,
    sender: Option<&str>,
    eids: &BTreeSet<String>,
) -> Result<EnvelopeBinding, ConsignmentError> {
    let seller_user_id =
        seller_by_name(connection, sender).await.map_err(WriteError::into_application)?;
    let mut reservations = Vec::new();
    let mut seasons = BTreeSet::new();
    if let Some(seller) = seller_user_id
        && !eids.is_empty()
    {
        let eids = eids.iter().cloned().collect::<Vec<_>>();
        let rows = sqlx::query(
            "SELECT r.id, r.listing_id, r.seller_user_id, r.world_id, r.canonical_eid,
                    r.generation, r.expires_at, r.state, r.season_window_id,
                    l.state_version
             FROM public.consignment_deposit_reservations AS r
             JOIN public.consignment_listings AS l ON l.id = r.listing_id
             WHERE r.world_id = $1 AND r.seller_user_id = $2 AND r.state = 'active'
               AND r.canonical_eid = ANY($3)
             ORDER BY r.canonical_eid, r.generation, r.id",
        )
        .bind(auction_domain::consignment::settlement_world_code(world))
        .bind(seller.as_uuid())
        .bind(&eids)
        .fetch_all(&mut *connection)
        .await
        .map_err(preflight_unavailable)?;
        for row in rows {
            let season: Option<Uuid> =
                row.try_get("season_window_id").map_err(preflight_unavailable)?;
            if let Some(season) = season {
                seasons.insert(season);
            }
            reservations.push(reservation_binding(&row).map_err(WriteError::into_application)?);
        }
    }
    if seasons.len() > 1 {
        return Err(ConsignmentError::Unavailable);
    }
    Ok(EnvelopeBinding {
        seller_user_id,
        reservations,
        season_window_id: seasons.into_iter().next(),
    })
}

impl PollEnvelopeStore for DatabaseStore {
    fn load_envelope_preflight(
        &self,
        key: OriginalMailKey,
        observation: EnvelopeObservation,
        sender: Option<String>,
    ) -> auction_application::consignment::ConsignmentFuture<'_, EnvelopePreflight> {
        Box::pin(async move {
            let mut connection = self.pool().acquire().await.map_err(preflight_unavailable)?;
            let now = database_now(&mut connection).await.map_err(WriteError::into_application)?;
            // D9 §2.2：候选只能引用世界就绪投影签收的那个会话；同 UID/world 的其它
            // 会话不能替代它，否则观察会把未签收的证据带进首次决定。
            let readiness = sqlx::query(
                "SELECT readiness.take_ready, readiness.return_ready, session.id AS session_id
                 FROM public.consignment_mail_readiness AS readiness
                 JOIN public.mail_evidence_sessions AS session
                   ON session.id = readiness.session_id
                  AND session.world_id = readiness.world_id
                 WHERE readiness.world_id = $1 AND readiness.detail_ready = true
                   AND readiness.valid_until > clock_timestamp()
                   AND session.bot_uid = $2 AND session.binding_digest = $3",
            )
            .bind(auction_domain::consignment::settlement_world_code(key.world()))
            .bind(key.authenticated_bot_uid())
            .bind(observation.session_binding_digest.as_slice())
            .fetch_optional(&mut *connection)
            .await
            .map_err(preflight_unavailable)?;
            let Some(readiness) = readiness else {
                return Err(ConsignmentError::Unavailable);
            };
            let session_id: Uuid =
                readiness.try_get("session_id").map_err(preflight_unavailable)?;
            let binding = read_envelope_binding(
                &mut connection,
                key.world(),
                sender.as_deref(),
                &canonical_eids(&observation),
            )
            .await?;
            let existing_decision = read_existing_decision(&mut connection, &key)
                .await
                .map_err(WriteError::into_application)?;
            let capabilities = EnvelopeCapabilities {
                safe_original_return: readiness
                    .try_get("return_ready")
                    .map_err(preflight_unavailable)?,
                take_ready: readiness.try_get("take_ready").map_err(preflight_unavailable)?,
            };
            Ok(EnvelopePreflight {
                seller_user_id: binding.seller_user_id,
                reservations: binding.reservations,
                existing_decision,
                session_id,
                capabilities,
                now,
                season_window_id: binding.season_window_id,
            })
        })
    }

    fn decide_envelope_in_poll(
        &self,
        write: PollEnvelopeWrite,
    ) -> auction_application::consignment::ConsignmentFuture<'_, EnvelopeReceipt> {
        Box::pin(async move {
            if write.command.is_some() != write.raw.is_some()
                || write
                    .command
                    .as_ref()
                    .is_some_and(|command| command.command_id != write.command_id)
                || write.cursor_mid.is_empty()
                || write.cursor_mid != write.cursor_mid.trim()
                || write.raw.as_ref().is_some_and(|raw| raw.mail_key != write.mail_key)
                || write.raw.as_ref().is_some_and(|raw| raw.mailbox_key != write.claim.mailbox_key)
                || write.claim.mailbox_key.is_empty()
                || write.next_poll_at <= write.polled_at
            {
                return Err(ConsignmentError::InvalidDepositRequest);
            }
            let sellers =
                write.command.as_ref().map_or_default(|command| envelope_sellers(&command.facts));
            let equipment =
                write.command.as_ref().map_or_default(|command| envelope_equipment(&command.facts));
            let key = write.mail_key.clone();
            let keys = LockKeys {
                command_id: write.command_id.as_uuid(),
                sellers: &sellers,
                equipment: &equipment,
            };
            let mail_keys = [key.clone()];
            let input = write;
            let transaction =
                with_deposit_transaction(self, keys, &mail_keys, &input, |input, tx| {
                    let key = key.clone();
                    Box::pin(async move {
                        if let Some(command) = &input.command {
                            // D9 §1.3：锁键集合在预读后变化时必须退出本事务，由调用方以同一
                            // command_id 原样重试并重新预读；不能在锁内补锁或沿用旧快照。
                            let raw = input.raw.as_ref().ok_or_else(|| {
                                WriteError::from(ConsignmentError::InvalidDepositRequest)
                            })?;
                            let binding = read_envelope_binding(
                                tx.connection(),
                                key.world(),
                                raw.sender.as_deref(),
                                &canonical_eids(&command.facts.observation),
                            )
                            .await
                            .map_err(WriteError::from)?;
                            let expected_reservations = command
                                .facts
                                .reservations
                                .iter()
                                .map(|reservation| reservation.reservation_id)
                                .collect::<BTreeSet<_>>();
                            let current_reservations = binding
                                .reservations
                                .iter()
                                .map(|reservation| reservation.reservation_id)
                                .collect::<BTreeSet<_>>();
                            if binding.seller_user_id != command.facts.envelope_seller_user_id
                                || current_reservations != expected_reservations
                            {
                                return Err(WriteError::from(ConsignmentError::Unavailable));
                            }
                        }
                        // 已有决定优先：不重写观察、命令或工作，只推进前缀并释放租约。
                        if let Some(receipt) = read_existing_decision(tx.connection(), &key).await?
                        {
                            if receipt.command_id != input.command_id {
                                return Err(WriteError::from(ConsignmentError::CommandIdConflict));
                            }
                            advance_poll_prefix(tx, input).await?;
                            return Ok(receipt);
                        }
                        let receipt = match &input.command {
                            Some(command) => {
                                match tx
                                    .arbitrate(
                                        DepositActor::Worker,
                                        DepositOperation::DecideEnvelope,
                                        DepositTarget::MailKey(key.clone()),
                                        command.request_hash,
                                        None,
                                    )
                                    .await?
                                {
                                    DepositArbitration::Replay(stored) => {
                                        match replay_receipt::<EnvelopeReceipt>(&stored)? {
                                            crate::database::mail::originals::Outcome::Accepted(
                                                receipt,
                                            ) => receipt,
                                            crate::database::mail::originals::Outcome::Rejected(
                                                _,
                                            ) => {
                                                return Err(WriteError::from(
                                                    ConsignmentError::Unavailable,
                                                ));
                                            }
                                        }
                                    }
                                    DepositArbitration::First(mut writer) => {
                                        let now = database_now(writer.connection()).await?;
                                        let raw = input.raw.as_ref().ok_or_else(|| {
                                            WriteError::from(
                                                ConsignmentError::InvalidDepositRequest,
                                            )
                                        })?;
                                        observations::record_observation_in_tx(
                                            writer.connection(),
                                            &observations::RawObservation {
                                                mail_key: &raw.mail_key,
                                                session_id: raw.session_id,
                                                mailbox_key: &raw.mailbox_key,
                                                remote_observation_id: &raw.remote_observation_id,
                                                observed_at: raw.observed_at,
                                                parser_version: &raw.parser_version,
                                                evidence_ref: raw.evidence_ref.as_deref(),
                                                subject_digest: raw.subject_digest,
                                                body_digest: raw.body_digest,
                                                sender: raw.sender.as_deref(),
                                                recipient: raw.recipient.as_deref(),
                                                raw_count: raw.raw_count,
                                                observation: &raw.observation,
                                                legacy_evidence: raw.legacy_evidence.clone(),
                                                received_minute_raw: &raw.received_minute_raw,
                                                received_timezone: &raw.received_timezone,
                                                first_seen_at: raw.first_seen_at,
                                                season_window_id: raw.season_window_id,
                                            },
                                        )
                                        .await?;
                                        let receipt =
                                            decide_envelope_in_tx(&mut writer, command, now)
                                                .await?;
                                        let _ = writer.finish(receipt.clone(), now).await?;
                                        receipt
                                    }
                                }
                            }
                            None => return Err(WriteError::from(ConsignmentError::Unavailable)),
                        };
                        advance_poll_prefix(tx, input).await?;
                        Ok(receipt)
                    })
                });
            transaction.await.map_err(WriteError::into_application)
        })
    }
}

async fn advance_poll_prefix(
    tx: &mut DepositTransaction<'_, '_>,
    write: &PollEnvelopeWrite,
) -> Result<(), WriteError> {
    complete_prefix(tx.tracked(), &write.claim.mailbox_key, Some(write.cursor_mid.as_str()))
        .await
        .map_err(prefix_unavailable)?;
    advance_prefix_in_tx(
        tx.connection(),
        &PrefixClaim {
            mailbox_key: &write.claim.mailbox_key,
            claim_token: write.claim.claim_token,
            claim_owner: &write.claim.worker_id,
            state_version: write.claim.state_version,
            expected_cursor: write.claim.cursor_value.as_deref(),
            next_cursor: &write.cursor_mid,
            polled_at: write.polled_at,
            next_poll_at: write.next_poll_at,
        },
    )
    .await
    .map_err(|error| WriteError::from(error.into_application()))
}

/// 按 K 读取首次决定的持久回执；只解码账本，不重新分类信封。
pub(crate) async fn read_existing_decision(
    connection: &mut PgConnection,
    key: &OriginalMailKey,
) -> Result<Option<EnvelopeReceipt>, WriteError> {
    let columns = MailKeyColumns::from_key(key);
    let row = sqlx::query(
        "SELECT c.result_status, c.result_code, c.result_body
         FROM public.mail_initial_decisions AS decision
         JOIN public.consignment_commands AS c ON c.command_id = decision.command_id
         WHERE decision.bot_uid = $1 AND decision.world_id = $2
           AND decision.external_mid = $3",
    )
    .bind(columns.bot_uid)
    .bind(columns.world_id)
    .bind(columns.external_mid)
    .fetch_optional(&mut *connection)
    .await?;
    let Some(row) = row else { return Ok(None) };
    let stored = StoredReceipt {
        status: row.try_get("result_status")?,
        code: row.try_get("result_code")?,
        body: row.try_get("result_body")?,
    };
    match replay_receipt::<EnvelopeReceipt>(&stored)? {
        Outcome::Accepted(receipt) => Ok(Some(receipt)),
        Outcome::Rejected(_) => Err(crate::database::mail::originals::invariant(
            "consignment_initial_decision_rejected",
        )),
    }
}

#[cfg(feature = "test-db")]
impl crate::database::consignment::deposit::repository::DepositRepository {
    /// PG 测试入口：按 K 读取首次决定回执。
    pub async fn test_read_existing_decision(
        &self,
        key: &OriginalMailKey,
    ) -> Result<Option<EnvelopeReceipt>, String> {
        let mut connection =
            self.store().pool().acquire().await.map_err(|error| format!("{error:?}"))?;
        read_existing_decision(&mut connection, key).await.map_err(|error| format!("{error:?}"))
    }

    /// PG 测试入口：在调用方持有的同一事务里执行前缀 CAS。
    #[allow(clippy::too_many_arguments, reason = "测试逐项传入租约与游标")]
    pub async fn test_advance_prefix(
        &self,
        connection: &mut PgConnection,
        mailbox_key: &str,
        claim_token: Uuid,
        claim_owner: &str,
        state_version: i64,
        expected_cursor: Option<&str>,
        next_cursor: &str,
        polled_at: Timestamp,
        next_poll_at: Timestamp,
    ) -> Result<(), String> {
        advance_prefix_in_tx(
            connection,
            &PrefixClaim {
                mailbox_key,
                claim_token,
                claim_owner,
                state_version,
                expected_cursor,
                next_cursor,
                polled_at,
                next_poll_at,
            },
        )
        .await
        .map_err(|error| format!("{error:?}"))
    }
}
