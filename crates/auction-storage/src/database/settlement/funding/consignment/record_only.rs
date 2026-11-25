//! 收款的模式规则：严格现金与 record_only 的差异集中在这里。
//!
//! 保护不变量 6、7：账户模式一经建立不可切换，Sale 冻结的模式、命令声明的模式与账户当前模式三者必须一致；
//! record_only 不虚设 opening、不读取个人余额，同样必须有远端付款明细；载体本金 1 在严格现金下归原 UID 自有，
//! 在 record_only 下进不可支用的核销子账（不可 transfer、reserve、再发行）。

use auction_application::settlement::FundingError;
use auction_domain::consignment::funding::FundingMode;
use sqlx::Postgres;
use uuid::Uuid;

use super::distribution::contract_allocation;
use crate::database::TrackedTransaction as Transaction;
use crate::database::settlement::funding::account::LockedFundingAccount;
use crate::database::settlement::funding::ledger::allocation;

/// 账户模式必须与命令声明的模式一致；不一致不换模式、不放行。
pub(super) fn check_mode(
    account: &LockedFundingAccount,
    mode: FundingMode,
) -> Result<(), FundingError> {
    let account_mode =
        if account.records_only() { FundingMode::RecordOnly } else { FundingMode::StrictCash };
    if account_mode == mode { Ok(()) } else { Err(FundingError::Conflict) }
}

/// 载体本金 1 的最终去向：严格现金归原 UID 的自有用途，record_only 进合同的核销子账。
pub(super) async fn principal_destination(
    tx: &mut Transaction<'_, Postgres>,
    uid: &str,
    mode: FundingMode,
    contract_id: Uuid,
) -> Result<Uuid, FundingError> {
    match mode {
        FundingMode::StrictCash => allocation(tx, uid, "proprietary", None, None).await,
        FundingMode::RecordOnly => {
            contract_allocation(tx, uid, "carrier_writeoff", contract_id, None).await
        }
    }
}
