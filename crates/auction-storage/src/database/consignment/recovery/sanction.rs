//! 寄售处罚执行器（第 7 期 7B-2）：接管第 6 期固定的处罚与账号工作，执行账号效果并追加不可改写的效果事实。
//!
//! 保护的不变量：
//! - 1、5：账号命令 ID 与摘要只由处罚业务键派生，效果行 `UNIQUE(sanction_id)`；重放、重启、并发领取都落到
//!   同一条账号命令与同一行效果，工作状态与结果由数据库持久，内存定时器只负责唤醒。
//! - 2：整个执行在买家用户锁（与购买、拒收、解禁共用）下进行，锁跨过 Users 拉黑命令的独立事务，
//!   解禁因此不会插在“决定执行”与“账号被禁”之间。锁与快照的次序同寄售命令：先取会话锁，再开 Serializable。
//! - 6：入参不含窗口、阈值与计数，决定只来自 [`decide_sanction_execution`]；`Applied` 却无回执等
//!   矛盾事实失败关闭，转人工对账（[`SanctionWorkOutcome::Inconsistent`]），不补写、不重复执行。
//! - 8：效果与结果只追加；处罚与账号工作投影只前进一步。

use auction_application::admin::{RecoveryBuyerBlacklistCommand, RecoveryBuyerBlacklister};
use auction_application::consignment::{
    ConsignmentError, ConsignmentFuture, ConsignmentSanctionAccountWork, RecoveryOperation,
    SanctionWorkOutcome, SanctionWorkRef, SanctionWorkStore, apply_sanction_command,
    consignment_sanction_blacklist_command,
};
use auction_domain::consignment::recovery::{
    SanctionExecution, SanctionExecutionFacts, SanctionWorkState, UserStatusFact,
    decide_sanction_execution,
};
use auction_domain::consignment::refusal::SanctionRule;
use auction_time::Timestamp;
use futures_util::future::BoxFuture;
use sqlx::postgres::Postgres;
use sqlx::{Connection, PgConnection};
use uuid::Uuid;

use crate::database::DatabaseStore;
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::commands::session::user_lock_key;
use crate::database::consignment::deposit::primitives::database_now;
use crate::database::consignment::recovery::ledger::{
    Arbitration, LedgerCommand, Step, Target, arbitrate, insert_success,
};
use crate::database::pool::TrackedConnection;

/// 每个阶段遇到序列化冲突时的有界重试次数；阶段本身幂等。
const PHASE_ATTEMPTS: usize = 3;

/// 工作与处罚联查的一行：买家、处罚、工作状态、处罚状态、规则、判定时刻、周期是否 open、效果、账号状态。
type FactsRow = (Uuid, Uuid, String, String, String, Timestamp, bool, Option<Uuid>, String);

/// 锁内读到的账号工作事实。
struct WorkFacts {
    buyer_user_id: Uuid,
    sanction_id: Uuid,
    work_state: String,
    sanction_state: String,
    rule: String,
    t0: Timestamp,
    cycle_open: bool,
    effect: Option<Uuid>,
    user_status: String,
}

/// 第一阶段的结论：在本阶段内已收口，或需要先落账号命令。
enum Plan {
    Done(SanctionWorkOutcome),
    Blacklist { command: Box<RecoveryBuyerBlacklistCommand>, user_status: UserStatusFact },
}

impl WorkFacts {
    fn work_state(&self) -> Result<SanctionWorkState, WriteError> {
        match self.work_state.as_str() {
            "pending" | "claimed" => Ok(SanctionWorkState::Pending),
            "completed" => Ok(SanctionWorkState::Applied),
            "cancelled" => Ok(SanctionWorkState::CycleClosed),
            _ => Err(WriteError::invariant("consignment_sanction_work_state")),
        }
    }

    fn user_status(&self) -> Result<UserStatusFact, WriteError> {
        match self.user_status.as_str() {
            "active" => Ok(UserStatusFact::Active),
            "blacklisted" => Ok(UserStatusFact::Blacklisted),
            _ => Err(WriteError::invariant("consignment_sanction_user_status")),
        }
    }

    fn rule(&self) -> Result<SanctionRule, WriteError> {
        match self.rule.as_str() {
            "main_cod_returns" => Ok(SanctionRule::MainCodReturns),
            "isekai_unpaid_carrier_returns" => Ok(SanctionRule::IsekaiUnpaidCarrierReturns),
            _ => Err(WriteError::invariant("consignment_sanction_rule")),
        }
    }

    fn execution_facts(
        &self,
        user_status: UserStatusFact,
    ) -> Result<SanctionExecutionFacts, WriteError> {
        Ok(SanctionExecutionFacts {
            work_state: self.work_state()?,
            cycle_still_open: self.cycle_open,
            effect_exists: self.effect,
            user_status,
        })
    }

    /// 由处罚业务键派生账号拉黑命令；命令 ID 与摘要不含工作 ID，工作重建不会产生第二条命令。
    fn blacklist_command(
        &self,
        work: SanctionWorkRef,
    ) -> Result<RecoveryBuyerBlacklistCommand, WriteError> {
        let account_work = ConsignmentSanctionAccountWork {
            sanction_id: self.sanction_id,
            work_id: work.work_id,
            buyer_user_id: self.buyer_user_id,
            rule: self.rule()?,
            occurred_at: self.t0,
        };
        consignment_sanction_blacklist_command(&account_work).map_err(|error| {
            tracing::warn!(%error, "寄售处罚账号命令无法构造");
            WriteError::invariant("consignment_sanction_command")
        })
    }
}

/// 读取并锁定工作与处罚行；工作与买家不符、行缺失都失败关闭。
async fn load_facts(
    conn: &mut PgConnection,
    work: SanctionWorkRef,
) -> Result<WorkFacts, WriteError> {
    let row: Option<FactsRow> = sqlx::query_as(
        "SELECT work.buyer_user_id, work.sanction_id, work.state, sanction.state, sanction.rule,
                    sanction.t0, cycle.state = 'open', effect.sanction_id, account.status
             FROM public.consignment_account_work AS work
             JOIN public.consignment_sanctions AS sanction ON sanction.id = work.sanction_id
             JOIN public.consignment_refusal_cycles AS cycle ON cycle.id = sanction.cycle_id
             JOIN public.users AS account ON account.id = work.buyer_user_id
             LEFT JOIN public.consignment_sanction_effects AS effect
               ON effect.sanction_id = sanction.id
             WHERE work.id = $1
             FOR UPDATE OF work, sanction",
    )
    .bind(work.work_id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some((
        buyer_user_id,
        sanction_id,
        work_state,
        sanction_state,
        rule,
        t0,
        cycle_open,
        effect,
        user_status,
    )) = row
    else {
        return Err(WriteError::invariant("consignment_sanction_work_missing"));
    };
    if buyer_user_id != work.buyer_user_id || sanction_id != work.sanction_id {
        return Err(WriteError::invariant("consignment_sanction_work_identity"));
    }
    Ok(WorkFacts {
        buyer_user_id,
        sanction_id,
        work_state,
        sanction_state,
        rule,
        t0,
        cycle_open,
        effect,
        user_status,
    })
}

/// 工作与处罚投影前进一步；`from` 不符时不动（由触发器与调用方的状态核对兜底）。
async fn advance_work(
    conn: &mut PgConnection,
    work_id: Uuid,
    from: &str,
    to: &str,
) -> Result<(), WriteError> {
    let moved = sqlx::query(
        "UPDATE public.consignment_account_work SET state = $3, version = version + 1
         WHERE id = $1 AND state = $2",
    )
    .bind(work_id)
    .bind(from)
    .bind(to)
    .execute(&mut *conn)
    .await?;
    if moved.rows_affected() == 1 {
        Ok(())
    } else {
        Err(WriteError::invariant("consignment_sanction_work_transition"))
    }
}

async fn advance_sanction(
    conn: &mut PgConnection,
    sanction_id: Uuid,
    to: &str,
) -> Result<(), WriteError> {
    let moved = sqlx::query(
        "UPDATE public.consignment_sanctions SET state = $2, version = version + 1
         WHERE id = $1 AND state = 'pending'",
    )
    .bind(sanction_id)
    .bind(to)
    .execute(&mut *conn)
    .await?;
    if moved.rows_affected() == 1 {
        Ok(())
    } else {
        Err(WriteError::invariant("consignment_sanction_transition"))
    }
}

/// 追加效果与结果行；与上面的投影前进必须在同一事务（延迟触发器在提交前核对一致）。
async fn append_effect(
    conn: &mut PgConnection,
    facts: &WorkFacts,
    work_id: Uuid,
    state: &str,
    account_command_id: Option<Uuid>,
    at: Option<Timestamp>,
) -> Result<(), WriteError> {
    let now = match at {
        Some(at) => at,
        None => database_now(conn).await?,
    };
    sqlx::query(
        "INSERT INTO public.consignment_sanction_effects
            (sanction_id, buyer_user_id, state, account_command_id, applied_at)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(facts.sanction_id)
    .bind(facts.buyer_user_id)
    .bind(state)
    .bind(account_command_id)
    .bind(now)
    .execute(&mut *conn)
    .await?;
    sqlx::query(
        "INSERT INTO public.consignment_sanction_work_results
            (work_id, sanction_id, result, recorded_at)
         VALUES ($1, $2, $3, $4)",
    )
    .bind(work_id)
    .bind(facts.sanction_id)
    .bind(state)
    .bind(now)
    .execute(&mut *conn)
    .await?;
    // 不变量 1、3：账本里的 apply_sanction 回执与效果同事务追加；同处罚重放只会回放这条回执。
    let (command_id, request_hash) = apply_sanction_command(facts.sanction_id, facts.buyer_user_id);
    let mut receipt =
        serde_json::json!({ "sanction_id": facts.sanction_id, "effect_state": state });
    if let (Some(account_command_id), Some(fields)) = (account_command_id, receipt.as_object_mut())
    {
        fields.insert("account_command_id".into(), serde_json::json!(account_command_id));
    }
    let ledger = LedgerCommand {
        command_id: command_id.as_uuid(),
        operation: RecoveryOperation::ApplySanction,
        target: Target::Sanction(facts.sanction_id),
        digest: &request_hash,
    };
    insert_success(conn, &ledger, "sanction_applied", receipt).await
}

/// 账号效果已落（`account_command_id` 为 `None` 表示账号已因别的途径被禁，只记录）：
/// 工作转 completed、处罚转 applied，追加效果与结果。
async fn complete_applied(
    conn: &mut PgConnection,
    facts: &WorkFacts,
    work_id: Uuid,
    account_command_id: Option<Uuid>,
    at: Option<Timestamp>,
) -> Result<(), WriteError> {
    if facts.work_state == "pending" {
        advance_work(conn, work_id, "pending", "claimed").await?;
    }
    advance_work(conn, work_id, "claimed", "completed").await?;
    advance_sanction(conn, facts.sanction_id, "applied").await?;
    let state = if account_command_id.is_some() { "applied" } else { "already_blacklisted" };
    append_effect(conn, facts, work_id, state, account_command_id, at).await
}

/// 周期已解禁：旧工作收口，不禁人。处罚若已被解禁路径转 `cycle_closed` 就只补效果与结果。
async fn complete_cycle_closed(
    conn: &mut PgConnection,
    facts: &WorkFacts,
    work_id: Uuid,
    at: Option<Timestamp>,
) -> Result<(), WriteError> {
    if facts.sanction_state == "pending" {
        advance_sanction(conn, facts.sanction_id, "cycle_closed").await?;
    }
    if facts.work_state != "cancelled" {
        advance_work(conn, work_id, &facts.work_state, "cancelled").await?;
    }
    append_effect(conn, facts, work_id, "cycle_closed", None, at).await
}

async fn blacklist_command_exists(
    conn: &mut PgConnection,
    command_id: Uuid,
    buyer_user_id: Uuid,
) -> Result<bool, WriteError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (
             SELECT 1 FROM public.user_admin_commands
             WHERE command_id = $1 AND command_kind = 'blacklist' AND user_id = $2)",
    )
    .bind(command_id)
    .bind(buyer_user_id)
    .fetch_one(&mut *conn)
    .await?)
}

/// 第一阶段：锁内读事实、按领域决定收口或给出账号命令。
async fn plan(
    conn: &mut PgConnection,
    work: SanctionWorkRef,
    at: Option<Timestamp>,
) -> Result<Plan, WriteError> {
    // 不变量 1：先仲裁账本。同命令 ID 异请求冲突；已有回执说明效果早已与回执同事务落库。
    let (command_id, request_hash) = apply_sanction_command(work.sanction_id, work.buyer_user_id);
    let ledger = LedgerCommand {
        command_id: command_id.as_uuid(),
        operation: RecoveryOperation::ApplySanction,
        target: Target::Sanction(work.sanction_id),
        digest: &request_hash,
    };
    match arbitrate(conn, &ledger).await? {
        Step::Done(Arbitration::Replay(_)) => {
            return Ok(Plan::Done(SanctionWorkOutcome::Replayed));
        }
        Step::Done(Arbitration::First) => {}
        Step::Rejected(rejection) => return Err(WriteError::from(rejection.into_error())),
    }
    let facts = load_facts(conn, work).await?;
    let status = facts.user_status()?;
    let execution = decide_sanction_execution(&facts.execution_facts(status)?);
    // 已执行或已收口之外，处罚投影必须仍是 pending（解禁收口的处罚除外），否则是矛盾事实。
    let projection_consistent = match execution {
        SanctionExecution::Apply | SanctionExecution::AlreadyBlacklistedRecordOnly => {
            facts.sanction_state == "pending"
        }
        SanctionExecution::CycleClosed => {
            matches!(facts.sanction_state.as_str(), "pending" | "cycle_closed")
        }
        SanctionExecution::ReuseEffect(_) | SanctionExecution::Inconsistent => true,
    };
    if !projection_consistent {
        return Ok(Plan::Done(SanctionWorkOutcome::Inconsistent));
    }
    match execution {
        SanctionExecution::ReuseEffect(_) => Ok(Plan::Done(SanctionWorkOutcome::Replayed)),
        SanctionExecution::Inconsistent => Ok(Plan::Done(SanctionWorkOutcome::Inconsistent)),
        // 解禁路径已收口、效果也已追加：只重放，不再写。
        SanctionExecution::CycleClosed if facts.effect.is_some() => {
            Ok(Plan::Done(SanctionWorkOutcome::Replayed))
        }
        SanctionExecution::CycleClosed => {
            complete_cycle_closed(conn, &facts, work.work_id, at).await?;
            Ok(Plan::Done(SanctionWorkOutcome::CycleClosed))
        }
        SanctionExecution::Apply | SanctionExecution::AlreadyBlacklistedRecordOnly => {
            let command = facts.blacklist_command(work)?;
            let command_id = command.command_id;
            // 上次执行在账号命令提交后、效果提交前中断：命令已在账本里，直接登记效果，不再调用 Users。
            if blacklist_command_exists(conn, command_id, facts.buyer_user_id).await? {
                complete_applied(conn, &facts, work.work_id, Some(command_id), at).await?;
                return Ok(Plan::Done(SanctionWorkOutcome::Applied {
                    account_command_id: command_id,
                }));
            }
            if execution == SanctionExecution::AlreadyBlacklistedRecordOnly {
                // 账号因别的途径已被禁（统筹方 11-24 裁定）：效果记“已禁、仅记录”，不引用任何拉黑命令、
                // 不追加黑名单历史；本处罚自己的命令不在账本里，所以也不得引用买家名下其他命令。
                complete_applied(conn, &facts, work.work_id, None, at).await?;
                return Ok(Plan::Done(SanctionWorkOutcome::AlreadyBlacklisted));
            }
            if facts.work_state == "pending" {
                advance_work(conn, work.work_id, "pending", "claimed").await?;
            }
            Ok(Plan::Blacklist { command: Box::new(command), user_status: status })
        }
    }
}

/// 第二阶段：账号命令已提交，锁内重读并追加效果；周期或工作状态在锁内变了就失败关闭。
async fn finish_apply(
    conn: &mut PgConnection,
    work: SanctionWorkRef,
    user_status: UserStatusFact,
    account_command_id: Uuid,
    at: Option<Timestamp>,
) -> Result<SanctionWorkOutcome, WriteError> {
    let facts = load_facts(conn, work).await?;
    // 沿用第一阶段观察到的账号状态：命令提交后账号必然已是 blacklisted，不能再拿它重新判。
    match decide_sanction_execution(&facts.execution_facts(user_status)?) {
        SanctionExecution::Apply if facts.sanction_state == "pending" => {
            complete_applied(conn, &facts, work.work_id, Some(account_command_id), at).await?;
            Ok(SanctionWorkOutcome::Applied { account_command_id })
        }
        SanctionExecution::ReuseEffect(_) => Ok(SanctionWorkOutcome::Replayed),
        _ => Ok(SanctionWorkOutcome::Inconsistent),
    }
}

/// 持有买家会话锁的专用连接；`close_on_drop` 保证任何中断都以关闭连接解除锁。
struct BuyerLocked {
    connection: TrackedConnection<Postgres>,
    key: (i32, i32),
}

impl BuyerLocked {
    async fn acquire(store: &DatabaseStore, buyer: Uuid) -> Result<Self, WriteError> {
        let mut connection = store.pool().acquire().await?;
        connection.close_on_drop();
        sqlx::query(
            "SELECT set_config('statement_timeout', '5s', false),
                    set_config('lock_timeout', '1s', false)",
        )
        .execute(&mut *connection)
        .await?;
        let key = user_lock_key(buyer);
        sqlx::query("SELECT pg_advisory_lock($1, $2)")
            .bind(key.0)
            .bind(key.1)
            .execute(&mut *connection)
            .await?;
        Ok(Self { connection, key })
    }

    /// 顶层 Serializable 事务；序列化冲突整阶段有界重试，其他失败回滚后原样返回。
    async fn phase<T>(
        &mut self,
        operation: impl for<'c> Fn(&'c mut PgConnection) -> BoxFuture<'c, Result<T, WriteError>>,
    ) -> Result<T, WriteError> {
        let mut attempt = 1;
        loop {
            let mut tx = self.connection.begin_with("BEGIN ISOLATION LEVEL SERIALIZABLE").await?;
            let result = operation(&mut tx).await;
            let result = match result {
                Ok(value) => tx.commit().await.map(|()| value).map_err(WriteError::from),
                Err(error) => {
                    if let Err(rollback) = tx.rollback().await {
                        tracing::warn!(error = %rollback, "无法确认处罚执行事务已回滚，连接将关闭");
                    }
                    Err(error)
                }
            };
            match result {
                Err(WriteError::RetryConflict) if attempt < PHASE_ATTEMPTS => attempt += 1,
                other => return other,
            }
        }
    }

    async fn release(mut self) {
        let released = sqlx::query_scalar::<_, bool>("SELECT pg_advisory_unlock($1, $2)")
            .bind(self.key.0)
            .bind(self.key.1)
            .fetch_one(&mut *self.connection)
            .await;
        if !matches!(released, Ok(true)) {
            tracing::warn!("无法确认买家会话锁已解除，连接将关闭");
        }
    }
}

impl DatabaseStore {
    async fn execute_sanction_work_locked(
        &self,
        work: SanctionWorkRef,
        at: Option<Timestamp>,
    ) -> Result<SanctionWorkOutcome, ConsignmentError> {
        let mut locked = BuyerLocked::acquire(self, work.buyer_user_id)
            .await
            .map_err(WriteError::into_application)?;
        let outcome = self.run_sanction_phases(&mut locked, work, at).await;
        locked.release().await;
        outcome.map_err(WriteError::into_application)
    }

    async fn run_sanction_phases(
        &self,
        locked: &mut BuyerLocked,
        work: SanctionWorkRef,
        at: Option<Timestamp>,
    ) -> Result<SanctionWorkOutcome, WriteError> {
        let planned = locked.phase(|conn| Box::pin(plan(conn, work, at))).await?;
        let (command, user_status) = match planned {
            Plan::Done(outcome) => return Ok(outcome),
            Plan::Blacklist { command, user_status } => (command, user_status),
        };
        let expected_command_id = command.command_id;
        // 账号命令自带幂等：同一处罚的重放返回原回执。外部不可见的写在买家锁内完成后才追加效果；
        // 失败时工作停在 claimed，下一轮重入（命令已提交则第一阶段直接登记效果）。
        let result = self.ensure_buyer_blacklisted(*command).await.map_err(|error| {
            tracing::warn!(%error, "寄售处罚账号拉黑命令失败，工作保留待重入");
            WriteError::from(ConsignmentError::Unavailable)
        })?;
        if result.command_id != expected_command_id || result.buyer_user_id != work.buyer_user_id {
            return Ok(SanctionWorkOutcome::Inconsistent);
        }
        locked
            .phase(|conn| Box::pin(finish_apply(conn, work, user_status, expected_command_id, at)))
            .await
    }
}

#[cfg(feature = "test-db")]
impl DatabaseStore {
    /// 测试入口：以给定时刻作为效果的落库时刻执行（复现“处罚固定后两天才接管”）；决定不读时钟。
    pub async fn test_execute_sanction_work_at(
        &self,
        work: SanctionWorkRef,
        at: Timestamp,
    ) -> Result<SanctionWorkOutcome, ConsignmentError> {
        self.execute_sanction_work_locked(work, Some(at)).await
    }
}

impl SanctionWorkStore for DatabaseStore {
    fn open_sanction_work(&self, limit: u32) -> ConsignmentFuture<'_, Vec<SanctionWorkRef>> {
        Box::pin(async move {
            let rows: Vec<(Uuid, Uuid, Uuid)> = sqlx::query_as(
                "SELECT work.id, work.sanction_id, work.buyer_user_id
                 FROM public.consignment_account_work AS work
                 WHERE work.state IN ('pending', 'claimed')
                    OR (work.state = 'cancelled' AND NOT EXISTS (
                        SELECT 1 FROM public.consignment_sanction_work_results AS result
                        WHERE result.work_id = work.id))
                    -- completed 却没有效果回执是不可能的状态：列出来让执行器失败关闭、转人工（7B-1 裁定）。
                    OR (work.state = 'completed' AND NOT EXISTS (
                        SELECT 1 FROM public.consignment_sanction_effects AS effect
                        WHERE effect.sanction_id = work.sanction_id))
                 ORDER BY work.created_at, work.id LIMIT $1",
            )
            .bind(i64::from(limit))
            .fetch_all(self.pool())
            .await
            .map_err(|error| WriteError::from(error).into_application())?;
            Ok(rows
                .into_iter()
                .map(|(work_id, sanction_id, buyer_user_id)| SanctionWorkRef {
                    work_id,
                    sanction_id,
                    buyer_user_id,
                })
                .collect())
        })
    }

    fn execute_sanction_work(
        &self,
        work: SanctionWorkRef,
    ) -> ConsignmentFuture<'_, SanctionWorkOutcome> {
        Box::pin(self.execute_sanction_work_locked(work, None))
    }
}
