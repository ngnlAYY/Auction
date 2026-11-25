//! 购前与首次派发前的只读资金预检。
//!
//! 不取资金锁、不写任何行；买后条件变化只保留权利、等待原 UID 核对。认证 Bot UID 不由调用方传入：
//! 或由装配注入（[`ConsignmentFundingPreflightReader`]），或每次从数据库的权威来源读取
//! （[`ActiveBotFundingPreflight`]）。预检的 `Ready` 与成交冻结必须同源（同一份事实）。

use auction_application::settlement::{
    ConsignmentFundingPreflight, FundingError, FundingFuture, FundingPreflightFact,
    FundingUnavailable,
};
use auction_domain::SettlementWorld;
use auction_domain::consignment::funding::FundingMode;
use auction_time::Timestamp;
use sqlx::Row;
use uuid::Uuid;

use super::contract_allocations::parse_mode;
use crate::DatabaseStore;
use crate::database::settlement::funding::storage;

/// 绑定认证 Bot UID 的预检读取器。
#[derive(Debug, Clone)]
pub struct ConsignmentFundingPreflightReader {
    store: DatabaseStore,
    uid: String,
}

impl ConsignmentFundingPreflightReader {
    /// 绑定存储与认证 Bot UID。
    pub fn new(store: DatabaseStore, uid: String) -> Self {
        Self { store, uid }
    }

    async fn read(&self) -> Result<FundingPreflightFact, FundingError> {
        let pool = self.store.pool();
        let account = sqlx::query(
            r#"SELECT status, accounting_mode, state_version, created_at
               FROM public.settlement_funding_accounts
               WHERE uid=$1"#,
        )
        .bind(&self.uid)
        .fetch_optional(pool)
        .await
        .map_err(storage)?;
        let Some(account) = account else {
            return Ok(unavailable(FundingUnavailable::AccountMissing));
        };
        let status: String = account.try_get("status").map_err(storage)?;
        match status.as_str() {
            "ready" => {}
            "frozen" => return Ok(unavailable(FundingUnavailable::Frozen)),
            _ => return Ok(unavailable(FundingUnavailable::NotReady)),
        }
        let Ok(mode) =
            parse_mode(&account.try_get::<String, _>("accounting_mode").map_err(storage)?)
        else {
            return Ok(unavailable(FundingUnavailable::ModeUnknown));
        };
        let blocked: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM public.consignment_cash_return_blocks WHERE uid=$1 AND state='open')",
        )
        .bind(&self.uid)
        .fetch_one(pool)
        .await
        .map_err(storage)?;
        if blocked {
            return Ok(unavailable(FundingUnavailable::OpeningBlocked));
        }
        // 全局规则：存在 record_only 账户时只能有这一个账户，严格现金账户之外不得混用。
        let others: i64 = sqlx::query_scalar(
            r#"SELECT count(*) FROM public.settlement_funding_accounts
               WHERE uid<>$1 AND (accounting_mode='record_only' OR status IN ('ready','frozen'))"#,
        )
        .bind(&self.uid)
        .fetch_one(pool)
        .await
        .map_err(storage)?;
        let record_only_elsewhere: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM public.settlement_funding_accounts WHERE uid<>$1 AND accounting_mode='record_only')",
        )
        .bind(&self.uid)
        .fetch_one(pool)
        .await
        .map_err(storage)?;
        if record_only_elsewhere || (mode == FundingMode::RecordOnly && others > 0) {
            return Ok(unavailable(FundingUnavailable::GlobalRecordOnlyConflict));
        }
        let boundary: Option<(Uuid, Timestamp)> = sqlx::query_as(
            r#"SELECT id, created_at FROM public.settlement_funding_sources
               WHERE uid=$1 AND kind IN ('reconciliation', 'record_opening')
               ORDER BY created_at DESC, id DESC
               LIMIT 1"#,
        )
        .bind(&self.uid)
        .fetch_optional(pool)
        .await
        .map_err(storage)?;
        // 原边界后没有现金 opening 的 record_only 不虚设 opening，以账户建立时刻为边界。
        let (boundary_source_id, boundary_at) = match (boundary, mode) {
            (Some((id, at)), _) => (Some(id), at),
            (None, FundingMode::RecordOnly) => {
                (None, account.try_get("created_at").map_err(storage)?)
            }
            (None, FundingMode::StrictCash) => {
                return Ok(unavailable(FundingUnavailable::BoundaryUnproven));
            }
        };
        Ok(FundingPreflightFact::Ready {
            uid: self.uid.clone(),
            mode,
            boundary_at,
            boundary_source_id,
            account_version: account.try_get("state_version").map_err(storage)?,
        })
    }
}

fn unavailable(reason: FundingUnavailable) -> FundingPreflightFact {
    FundingPreflightFact::Unavailable(reason)
}

impl ConsignmentFundingPreflight for ConsignmentFundingPreflightReader {
    fn preflight(&self, _world: SettlementWorld) -> FundingFuture<'_, FundingPreflightFact> {
        Box::pin(self.read())
    }
}

/// 每次预检从数据库权威来源读取认证 Bot 资金 UID 的预检器。
///
/// 资金一律在 main 世界（统筹方裁定：Bot UID/main、现金 main Credits），所以 UID 取 main 世界收件就绪行
/// 所指证据会话的 `bot_uid`，且该会话绑定的必须仍是当前 active、同版本的 main `mail_cookie` 凭据；
/// 不读配置、不硬编码、不在启动时缓存（凭据轮换后下一笔购买即取新 UID）。不要求就绪行仍在
/// `valid_until` 内（统筹方裁定）：UID 是认证 Bot 的身份事实，不随就绪心跳过期而改变；收件能力
/// 是否就绪由购买的其他门控各自判定。
/// 找不到时返回读取错误而不是 `Unavailable` 事实：购买因此整笔回滚为 503、不写回执，
/// 不伪造确定拒绝 `funding_unavailable`（不变量 4、6）。
#[derive(Debug, Clone)]
pub struct ActiveBotFundingPreflight {
    store: DatabaseStore,
}

impl ActiveBotFundingPreflight {
    /// 绑定存储；UID 在每次预检时读取。
    pub fn new(store: DatabaseStore) -> Self {
        Self { store }
    }

    async fn read(&self) -> Result<FundingPreflightFact, FundingError> {
        let uid: Option<String> = sqlx::query_scalar(
            r#"SELECT s.bot_uid
               FROM public.consignment_mail_readiness AS r
               JOIN public.mail_evidence_sessions AS s
                 ON s.id = r.session_id AND s.world_id = r.world_id
               JOIN public.credential_keys AS k ON k.id = s.credential_id
               WHERE r.world_id = 'main'
                 AND k.credential_kind = 'mail_cookie'
                 AND k.scope = 'main'
                 AND k.status = 'active'
                 AND k.state_version = s.credential_version"#,
        )
        .fetch_optional(self.store.pool())
        .await
        .map_err(storage)?;
        let Some(uid) = uid else {
            return Err(FundingError::Unavailable);
        };
        ConsignmentFundingPreflightReader::new(self.store.clone(), uid).read().await
    }
}

impl ConsignmentFundingPreflight for ActiveBotFundingPreflight {
    fn preflight(&self, _world: SettlementWorld) -> FundingFuture<'_, FundingPreflightFact> {
        Box::pin(self.read())
    }
}
