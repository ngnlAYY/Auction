//! 权利行与最新状态的读取、会话视图、续办与回执查询。
//!
//! 权利身份与定位符摘要不可变，版本 / 状态 / 动作范围在 `consignment_right_states` 里追加；
//! 本文件只读它们。续办在 `right` 与命令锁下核对会话、版本与动作范围，首次回执与拒绝回执
//! 同事务落账（不变量 1、2、6）。履约（同事务触发 6D 起写）由 7D-6 接入，本期没有任何状态会
//! 给出非空动作范围，因此 `claim_accepted` 路径在 7D-6 之前不可达，失败关闭。

use auction_application::consignment::{
    BrowserBinding, ClaimDisposition, ClaimOutcome, ClaimRightCommand, RightReceipt,
    RightsStoreError,
};
use auction_domain::consignment::rights::{
    RightAction, RightKind, RightTarget, RightView, SessionDigest,
};
use auction_domain::consignment::{parse_settlement_world, settlement_world_code};
use auction_domain::{CommandId, SettlementWorld};
use auction_time::Timestamp;
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::postgres::PgRow;
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use super::ledger::{self, LedgerKey, Subject};
use super::session::{self, SessionRow};
use super::{PgRightsStore, TxError, lock_key};

/// 权利行加最新状态版本。
pub(super) struct RightRow {
    pub(super) right_id: Uuid,
    pub(super) kind: RightKind,
    pub(super) world: SettlementWorld,
    pub(super) frozen_identity: String,
    pub(super) target: RightTarget,
    pub(super) version: u64,
    pub(super) state: String,
    pub(super) allowed: Vec<RightAction>,
}

impl RightRow {
    /// 只有 active 状态才有可执行动作。
    pub(super) fn is_active(&self) -> bool {
        self.state == "active"
    }

    /// 冻结身份摘要：世界与身份文本的域分隔 SHA-256，参与挑战 AAD。
    pub(super) fn identity_digest(&self) -> [u8; 32] {
        identity_digest(settlement_world_code(self.world), &self.frozen_identity)
    }
}

/// 冻结身份摘要的唯一实现。
pub(super) fn identity_digest(world: &str, frozen_identity: &str) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(b"auction.consignment.right.identity.v1\0");
    hash.update(world.as_bytes());
    hash.update([0]);
    hash.update(frozen_identity.as_bytes());
    hash.finalize().into()
}

/// 权利行加最新状态版本的 SELECT 片段；字面量宏，与 WHERE 子句 `concat!` 成静态 SQL。
macro_rules! right_select {
    () => {
        "SELECT r.right_id, r.kind, r.world_id, r.frozen_identity, r.contract_id,
        r.position_id, s.version, s.state, s.allowed_actions
 FROM public.consignment_rights AS r
 JOIN LATERAL (
     SELECT version, state, allowed_actions FROM public.consignment_right_states
     WHERE right_id = r.right_id ORDER BY version DESC LIMIT 1
 ) AS s ON true"
    };
}

fn corrupt() -> TxError {
    tracing::warn!("受限权利行形状损坏");
    TxError::Store(RightsStoreError::Unavailable)
}

fn parse_kind(text: &str) -> Option<RightKind> {
    [
        RightKind::ContinueOrder,
        RightKind::ClaimEquipment,
        RightKind::ClaimPayment,
        RightKind::WithdrawHeld,
        RightKind::LossCase,
    ]
    .into_iter()
    .find(|kind| kind.as_str() == text)
}

fn map_row(row: &PgRow) -> Result<RightRow, TxError> {
    let kind = parse_kind(row.try_get("kind")?).ok_or_else(corrupt)?;
    let world = parse_settlement_world(row.try_get("world_id")?).ok_or_else(corrupt)?;
    let contract: Option<Uuid> = row.try_get("contract_id")?;
    let position: Option<Uuid> = row.try_get("position_id")?;
    let target = match (contract, position) {
        (Some(contract_id), _) => RightTarget::Contract(contract_id),
        (None, Some(position_id)) => RightTarget::Position(position_id),
        (None, None) => return Err(corrupt()),
    };
    let allowed = row
        .try_get::<Vec<String>, _>("allowed_actions")?
        .iter()
        .map(|text| RightAction::parse(text))
        .collect::<Option<Vec<_>>>()
        .ok_or_else(corrupt)?;
    Ok(RightRow {
        right_id: row.try_get("right_id")?,
        kind,
        world,
        frozen_identity: row.try_get("frozen_identity")?,
        target,
        version: u64::try_from(row.try_get::<i64, _>("version")?).ok().ok_or_else(corrupt)?,
        state: row.try_get("state")?,
        allowed,
    })
}

/// 按权利 ID 读取。
pub(super) async fn by_id(
    connection: &mut PgConnection,
    right_id: Uuid,
) -> Result<Option<RightRow>, TxError> {
    let row = sqlx::query(concat!(right_select!(), " WHERE r.right_id = $1"))
        .bind(right_id)
        .fetch_optional(connection)
        .await?;
    row.as_ref().map(map_row).transpose()
}

/// 按定位符摘要读取；定位符只存摘要且全局唯一。
pub(super) async fn by_locator(
    connection: &mut PgConnection,
    digest: [u8; 32],
) -> Result<Option<RightRow>, TxError> {
    let row = sqlx::query(concat!(right_select!(), " WHERE r.locator_digest = $1"))
        .bind(digest.as_slice())
        .fetch_optional(connection)
        .await?;
    row.as_ref().map(map_row).transpose()
}

/// 按冻结身份与世界读取一项权利：有可执行状态的优先，其后按“最近一次定位指引最早者优先”轮转，
/// 使同一身份的多项权利轮流获得找回码。
pub(super) async fn by_identity(
    connection: &mut PgConnection,
    world: &str,
    frozen_identity: &str,
) -> Result<Option<RightRow>, TxError> {
    let row = sqlx::query(concat!(
        right_select!(),
        "
         LEFT JOIN LATERAL (
             SELECT max(created_at) AS last_guide FROM public.consignment_right_notice_outbox
             WHERE right_id = r.right_id AND notice_kind = 'locator_guide'
         ) AS guide ON true
         WHERE r.world_id = $1 AND r.frozen_identity = $2
         ORDER BY (s.state = 'active') DESC, guide.last_guide NULLS FIRST, r.created_at, r.right_id
         LIMIT 1"
    ))
    .bind(world)
    .bind(frozen_identity)
    .fetch_optional(connection)
    .await?;
    row.as_ref().map(map_row).transpose()
}

/// 会话授予的动作与当前状态允许的动作之交；状态不是 active 时为空。
fn effective_actions(row: &RightRow, scope: &[RightAction]) -> Vec<RightAction> {
    if !row.is_active() {
        return Vec::new();
    }
    row.allowed.iter().copied().filter(|action| scope.contains(action)).collect()
}

impl PgRightsStore {
    /// 会话读取：令牌非当前、浏览器不符、过期或未知都是 `None`。
    pub(super) async fn current_view(
        &self,
        session_digest: SessionDigest,
        browser: BrowserBinding,
    ) -> Result<Option<RightView>, RightsStoreError> {
        self.read(&(session_digest, browser), |(session_digest, browser), connection, now| {
            Box::pin(async move {
                let Some(session) =
                    session::authenticate(connection, *session_digest, *browser, now).await?
                else {
                    return Ok(None);
                };
                let row = by_id(connection, session.right_id).await?.ok_or_else(corrupt)?;
                Ok(Some(RightView {
                    right_id: row.right_id,
                    right_version: row.version,
                    kind: row.kind,
                    world: row.world,
                    contract_or_position: row.target,
                    allowed_actions: effective_actions(&row, &session.scope),
                }))
            })
        })
        .await
    }

    /// 续办。认证先于命令仲裁（与服务层一致）；首次与拒绝回执同事务落账。
    pub(super) async fn claim_right(
        &self,
        command: &ClaimRightCommand,
    ) -> Result<ClaimOutcome, RightsStoreError> {
        let key = LedgerKey {
            command_id: command.command_id.as_uuid(),
            subject: Subject::RightHolder(command.right_id),
            operation: "claim_right",
            request_digest: command.request_hash.as_bytes(),
        };
        let locks = vec![
            lock_key("command", key.command_id.as_bytes()),
            lock_key("right", command.right_id.as_bytes()),
        ];
        self.transact(locks, &(command, key), |(command, key), connection, now| {
            Box::pin(claim_in_tx(connection, now, command, *key))
        })
        .await
    }

    /// 本权利下的续办回执；原受益人重新验权后可查旧回执。
    pub(super) async fn claim_receipt(
        &self,
        session_digest: SessionDigest,
        browser: BrowserBinding,
        command_id: CommandId,
    ) -> Result<Option<RightReceipt>, RightsStoreError> {
        self.read(
            &(session_digest, browser, command_id),
            |(session_digest, browser, command_id), connection, now| {
                Box::pin(async move {
                    let Some(session) =
                        session::authenticate(connection, *session_digest, *browser, now).await?
                    else {
                        return Ok(None);
                    };
                    let row = sqlx::query(
                        "SELECT result_body FROM public.consignment_commands
                     WHERE command_id = $1 AND actor_kind = 'right_holder' AND actor_ref = $2
                       AND operation = 'claim_right' AND result_status < 400",
                    )
                    .bind(command_id.as_uuid())
                    .bind(session.right_id.to_string())
                    .fetch_optional(&mut *connection)
                    .await?;
                    let Some(row) = row else {
                        return Ok(None);
                    };
                    let receipt = ledger::StoredReceipt { body: row.try_get("result_body")? };
                    let outcome = outcome_from_receipt(&receipt)?;
                    Ok(Some(RightReceipt {
                        command_id: *command_id,
                        right_id: outcome.right_id,
                        disposition: outcome.disposition,
                    }))
                })
            },
        )
        .await
    }
}

fn disposition_from(text: &str) -> Option<ClaimDisposition> {
    [
        ClaimDisposition::ClaimAccepted,
        ClaimDisposition::AlreadyFulfilling,
        ClaimDisposition::AlreadyCompleted,
    ]
    .into_iter()
    .find(|disposition| disposition.as_str() == text)
}

fn outcome_from_receipt(receipt: &ledger::StoredReceipt) -> Result<ClaimOutcome, TxError> {
    let disposition = receipt.disposition().and_then(disposition_from).ok_or_else(corrupt)?;
    Ok(ClaimOutcome {
        disposition,
        right_id: receipt.uuid("right_id")?,
        right_version: receipt.version("right_version")?,
    })
}

async fn claim_in_tx(
    connection: &mut PgConnection,
    now: Timestamp,
    command: &ClaimRightCommand,
    key: LedgerKey,
) -> super::Persisted<ClaimOutcome> {
    // 不变量 2：先在锁内认证会话，令牌轮换、过期与浏览器绑定都以持久行为准。
    let session = session::authenticate(connection, command.session, command.browser, now)
        .await?
        .filter(|session: &SessionRow| session.right_id == command.right_id)
        .ok_or(RightsStoreError::Unauthenticated)?;
    if let Some(receipt) = ledger::arbitrate(connection, &key).await? {
        return Ok(match receipt.rejection() {
            Some(error) => Err(error),
            None => Ok(outcome_from_receipt(&receipt)?),
        });
    }
    let row = by_id(connection, command.right_id).await?.ok_or_else(corrupt)?;
    let rejection = if command.expected_version != row.version {
        Some(RightsStoreError::StaleVersion)
    } else if row.state == "fulfilled" {
        return accept(connection, now, key, &row, ClaimDisposition::AlreadyCompleted).await;
    } else if !row.is_active() {
        Some(RightsStoreError::RightExpired)
    } else if !effective_actions(&row, &session.scope).contains(&command.action) {
        Some(RightsStoreError::ActionNotAllowed)
    } else {
        None
    };
    if let Some(error) = rejection {
        ledger::record_rejection(connection, &key, error, now).await?;
        return Ok(Err(error));
    }
    let already: bool = sqlx::query_scalar(
        "SELECT EXISTS (
             SELECT 1 FROM public.consignment_commands
             WHERE actor_kind = 'right_holder' AND actor_ref = $1 AND operation = 'claim_right'
               AND result_body ->> 'disposition' = 'claim_accepted' AND command_id <> $2)",
    )
    .bind(row.right_id.to_string())
    .bind(key.command_id)
    .fetch_one(&mut *connection)
    .await?;
    let disposition =
        if already { ClaimDisposition::AlreadyFulfilling } else { ClaimDisposition::ClaimAccepted };
    accept(connection, now, key, &row, disposition).await
}

async fn accept(
    connection: &mut PgConnection,
    now: Timestamp,
    key: LedgerKey,
    row: &RightRow,
    disposition: ClaimDisposition,
) -> super::Persisted<ClaimOutcome> {
    let receipt = ledger::body(
        &key,
        disposition.as_str(),
        json!({ "right_id": row.right_id, "right_version": row.version }),
    );
    ledger::record(connection, &key, disposition.status(), "ok", receipt, now).await?;
    Ok(Ok(ClaimOutcome { disposition, right_id: row.right_id, right_version: row.version }))
}
