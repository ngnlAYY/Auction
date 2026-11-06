//! scope/account必须在业务锁之前取得，账户不存在也锁scope防启用竞态。

use super::diagnostics::{TARGET, reject};
use crate::database::TrackedTransaction as Transaction;
use crate::database::settlement_funding::{evidence_error, invalid, storage};
use auction_application::FundingError;
use sqlx::{Postgres, Row};
use uuid::Uuid;

/// 仅本owner可构造的早锁令牌；不得跨事务缓存或在提交后复用。
#[derive(Debug)]
pub struct LockedFundingAccount {
    pub(super) uid: String,
    pub(super) version: i64,
    pub(super) status: String,
    pub(super) accounting_mode: auction_application::FundingAccountingMode,
    pub(super) policy_version: i64,
    transaction_id: i64,
}

impl LockedFundingAccount {
    pub fn uid(&self) -> &str {
        &self.uid
    }
    pub fn version(&self) -> i64 {
        self.version
    }
    pub fn ready(&self) -> bool {
        self.status == "ready"
    }
    /// 只记录来源与责任，不把个人账号余额当作托管现金。
    pub fn records_only(&self) -> bool {
        self.accounting_mode == auction_application::FundingAccountingMode::RecordOnly
    }
    /// 固定策略版本独立于每次记账递增的账户版本。
    pub fn policy_version(&self) -> i64 {
        self.policy_version
    }
}

/// 防止提交后复用令牌，或同事务freeze后继续使用旧ready快照。
pub(super) async fn verify_locked(
    tx: &mut Transaction<'_, Postgres>,
    account: &LockedFundingAccount,
    require_ready: bool,
) -> Result<(), FundingError> {
    let (transaction_id, status): (i64, String) = sqlx::query_as(
        "SELECT txid_current(),status FROM public.settlement_funding_accounts WHERE uid=$1",
    )
    .bind(account.uid())
    .fetch_one(&mut **tx)
    .await
    .map_err(storage)
    .map_err(|error| reject("funding_account_verify_failed", "account_verify", error))?;
    if transaction_id != account.transaction_id {
        return Err(reject(
            "funding_account_transaction_conflict",
            "account_verify",
            FundingError::Conflict,
        ));
    }
    if require_ready && status != "ready" {
        return Err(reject(
            "funding_account_not_ready",
            "account_verify",
            FundingError::Unavailable,
        ));
    }
    Ok(())
}

pub(super) async fn lock_scope(tx: &mut Transaction<'_, Postgres>) -> Result<(), FundingError> {
    sqlx::query(
        "SELECT scope FROM public.settlement_funding_scope WHERE scope='mail_cookie' FOR UPDATE",
    )
    .fetch_one(&mut **tx)
    .await
    .map_err(storage)?;
    Ok(())
}

/// 调用前已锁generation/world；此函数不反向取得它们。
pub async fn lock_funding_account(
    tx: &mut Transaction<'_, Postgres>,
    uid: &str,
) -> Result<Option<LockedFundingAccount>, FundingError> {
    auction_domain::FundingAccountKey::new(uid).map_err(invalid)?;
    lock_scope(tx).await?;
    let row = sqlx::query("SELECT uid,state_version,status,accounting_mode,policy_version,txid_current() AS transaction_id FROM public.settlement_funding_accounts WHERE uid=$1 FOR UPDATE")
        .bind(uid).fetch_optional(&mut **tx).await.map_err(storage)?;
    row.map(|row| {
        Ok(LockedFundingAccount {
            uid: row.try_get("uid").map_err(storage)?,
            version: row.try_get("state_version").map_err(storage)?,
            status: row.try_get("status").map_err(storage)?,
            accounting_mode: super::policy::mode(
                &row.try_get::<String, _>("accounting_mode").map_err(storage)?,
            )?,
            policy_version: row.try_get("policy_version").map_err(storage)?,
            transaction_id: row.try_get("transaction_id").map_err(storage)?,
        })
    })
    .transpose()
}

/// 按持久出站分配定位原账户，供无当前远端UID的结果回写、过期claim及人工门禁使用；本函数不锁业务行。
/// 调用方先完成generation/world锁，随后才锁delivery并复核；所有allocation状态及返还关联均有效，
/// 账户冻结或未ready不影响定位；只有确无分配关联才返回None，scope锁保留到事务结束，
/// 不得以metadata替代本查询。
pub async fn lock_funded_delivery_account(
    tx: &mut Transaction<'_, Postgres>,
    delivery_id: Uuid,
) -> Result<Option<LockedFundingAccount>, FundingError> {
    if delivery_id.is_nil() {
        return Err(FundingError::Invalid);
    }
    lock_scope(tx).await?;
    // 不过滤state/amount、不内联JOIN账户，避免历史或孤儿关联被误判为未绑定。
    // UNIQUE约束正常保证一行；仍显式拒绝重复/多UID，损坏数据不能任选一个账户。
    let rows = sqlx::query(
        "SELECT a.uid, d.id AS existing_delivery_id
         FROM public.settlement_funding_allocations a
         LEFT JOIN public.deliveries d ON d.id=a.delivery_id
         WHERE a.delivery_id=$1 LIMIT 2",
    )
    .bind(delivery_id)
    .fetch_all(&mut **tx)
    .await
    .map_err(storage)?;
    let Some(row) = rows.first() else {
        // 返还证明必须仍有原allocation关联，不能因缺失而降级legacy。
        let orphan_return: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM public.settlement_funding_returns WHERE delivery_id=$1)",
        )
        .bind(delivery_id)
        .fetch_one(&mut **tx)
        .await
        .map_err(storage)?;
        return if orphan_return { Err(FundingError::Conflict) } else { Ok(None) };
    };
    if rows.len() != 1
        || row.try_get::<Option<Uuid>, _>("existing_delivery_id").map_err(storage)?
            != Some(delivery_id)
    {
        return Err(FundingError::Conflict);
    }
    let uid: String = row.try_get("uid").map_err(storage)?;
    // 复用规范UID校验与同transaction id令牌；已持scope，不会反向取得业务锁。
    let account = lock_funding_account(tx, &uid).await?.ok_or(FundingError::Conflict)?;
    Ok(Some(account))
}

/// 旧无授权入口必须在delivery锁之前调用；冻结仍保留保护。
pub async fn funding_requires_authorization(
    tx: &mut Transaction<'_, Postgres>,
) -> Result<bool, FundingError> {
    lock_scope(tx).await?;
    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM public.settlement_funding_accounts WHERE status IN ('ready','frozen'))")
        .fetch_one(&mut **tx).await.map_err(storage)
}

/// 返回false时调用方提交freeze但不发送；unknown永不按余额/TTL释放。
pub async fn check_funding_cash(
    tx: &mut Transaction<'_, Postgres>,
    account: &LockedFundingAccount,
    observed_credits: i64,
) -> Result<bool, FundingError> {
    verify_locked(tx, account, true).await?;
    if account.records_only() {
        return Err(reject(
            "funding_cash_mode_unsupported",
            "cash_gate",
            FundingError::Unsupported,
        ));
    }
    if observed_credits < 0 {
        return Err(reject("funding_cash_invalid", "cash_gate", FundingError::Invalid));
    }
    let (local, unknown): (i64, bool) = sqlx::query_as(
        "SELECT COALESCE(sum(amount) FILTER (WHERE state <> 'spent'),0)::bigint, COALESCE(bool_or(state IN ('unknown','in_flight')),false) FROM public.settlement_funding_allocations WHERE uid=$1")
        .bind(account.uid()).fetch_one(&mut **tx).await.map_err(storage)
        .map_err(|error| reject("funding_cash_query_failed", "cash_gate", error))?;
    if observed_credits < local {
        sqlx::query("UPDATE public.settlement_funding_accounts SET status='frozen',state_version=state_version+1,updated_at=clock_timestamp() WHERE uid=$1")
            .bind(account.uid()).execute(&mut **tx).await.map_err(storage)
            .map_err(|error| reject("funding_cash_freeze_failed", "cash_gate", error))?;
        tracing::warn!(target: TARGET, safe_kind = "funding_cash_shortfall", stage = "cash_gate", "现金缺口已写入原事务，等待原提交结果");
        return Ok(false);
    }
    if unknown {
        tracing::warn!(target: TARGET, safe_kind = "funding_outbound_unresolved", stage = "cash_gate", "原资金门存在未收口的外发占用");
    }
    Ok(!unknown)
}

/// 最终事务按已锁持久策略复核证据形状；record_only不采集/匹配余额，也不因其他lot未知而全局停账。
pub async fn check_funding_evidence(
    tx: &mut Transaction<'_, Postgres>,
    account: &LockedFundingAccount,
    evidence: &auction_application::SettlementInternalEvidenceSnapshot,
) -> Result<bool, FundingError> {
    verify_locked(tx, account, true).await?;
    if evidence.identity.authenticated_uid != account.uid() {
        return Err(reject(
            "funding_evidence_account_mismatch",
            "funding_evidence",
            FundingError::Unproven,
        ));
    }
    if !evidence.validate() {
        return Err(reject(
            "funding_evidence_snapshot_invalid",
            "funding_evidence",
            FundingError::Unproven,
        ));
    }
    crate::validate_mail_evidence_time(tx, evidence)
        .await
        .map_err(evidence_error)
        .map_err(|error| reject("funding_evidence_time_unproven", "funding_evidence", error))?;
    if account.records_only() {
        if evidence.main_cash.is_some() {
            return Err(reject(
                "funding_evidence_cash_unexpected",
                "funding_evidence",
                FundingError::Unproven,
            ));
        }
        // 原allocation未知状态仍由reserve/finish/return按原delivery精确保留，不自动释放。
        return Ok(true);
    }
    let cash = evidence.main_cash.as_ref().ok_or_else(|| {
        reject("funding_evidence_cash_missing", "funding_evidence", FundingError::Unproven)
    })?;
    check_funding_cash(tx, account, cash.credits).await
}
