//! 只读持久策略投影；record_only固定真实UID，默认旧strict，不从配置暗中降级。

use super::*;
use auction_application::{
    FundingAccountingMode, FundingFuture, FundingPolicy, FundingPolicyReader,
};

pub(super) fn mode(value: &str) -> Result<FundingAccountingMode, FundingError> {
    match value {
        "strict_cash" => Ok(FundingAccountingMode::StrictCash),
        "record_only" => Ok(FundingAccountingMode::RecordOnly),
        _ => Err(FundingError::Unproven),
    }
}

impl FundingPolicyReader for crate::DatabaseStore {
    fn funding_policy(&self) -> FundingFuture<'_, FundingPolicy> {
        Box::pin(async move {
            let rows=sqlx::query("SELECT uid,accounting_mode,policy_version FROM public.settlement_funding_accounts WHERE accounting_mode='record_only' OR status IN ('ready','frozen') ORDER BY uid")
                .fetch_all(self.postgres_pool()).await.map_err(storage)?;
            let records: Vec<_> = rows
                .iter()
                .filter(|row| {
                    row.try_get::<String, _>("accounting_mode").ok().as_deref()
                        == Some("record_only")
                })
                .collect();
            if let Some(row) = records.first() {
                if records.len() != 1 || rows.len() != 1 {
                    return Err(FundingError::Conflict);
                }
                return Ok(FundingPolicy {
                    mode: FundingAccountingMode::RecordOnly,
                    expected_uid: Some(row.try_get("uid").map_err(storage)?),
                    policy_version: row.try_get("policy_version").map_err(storage)?,
                });
            }
            Ok(FundingPolicy {
                mode: FundingAccountingMode::StrictCash,
                expected_uid: None,
                policy_version: 0,
            })
        })
    }
}
