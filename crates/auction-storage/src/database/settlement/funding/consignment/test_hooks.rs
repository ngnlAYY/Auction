//! `test-db` 下暴露给 PG 用例的入口：在运行账号权限下驱动本目录的 `pub(crate)` 原语与存储入口。
//!
//! 事务内原语（Take 起写建阻断、消费回补、手续费 earned）需要与调用方的同事务前置写入一起执行，
//! 因此入口接收一段 `setup_sql`，在取得 funding worlds 锁之后、调用原语之前于同一事务内执行。
//! 生产构建不含本模块。

use auction_application::settlement::{
    CashReceipt, ConvertRefund, FundingError, FundingMutation, RefundReceipt, SettleCash,
};
use auction_domain::consignment::ListingQuote;
use auction_domain::consignment::funding::{
    BlockedLeg, FundingMode, OpeningBlock, PrincipalConclusion,
};
use sqlx::Postgres;
use uuid::Uuid;

use super::blockers::open_cash_return_block_in_tx;
use super::cancel_unpaid::{CancelUnpaid, cancel_unpaid_in_tx};
use super::cash_legs::settle_cash;
use super::fee::write_fee_earned_in_tx;
use super::principal::{PrincipalFact, record_principal_fact_in_tx};
use super::refund_conversion::convert_refund;
use super::returns::{ReturnConsumptionFacts, return_cash_in_tx};
use super::transaction::run_serializable;
use crate::DatabaseStore;
use crate::database::TrackedTransaction as Transaction;
use crate::database::settlement::funding::{lock_funding_worlds, storage};

/// 在同一事务内先执行调用方的前置写入。
async fn run_setup(
    tx: &mut Transaction<'_, Postgres>,
    setup_sql: &str,
) -> Result<(), FundingError> {
    lock_funding_worlds(tx).await?;
    if setup_sql.is_empty() {
        return Ok(());
    }
    sqlx::raw_sql(sqlx::AssertSqlSafe(setup_sql.to_owned()))
        .execute(&mut **tx)
        .await
        .map_err(storage)?;
    Ok(())
}

impl DatabaseStore {
    /// PG 测试入口：现金腿状态迁移。
    pub async fn test_settle_cash(&self, command: SettleCash) -> Result<CashReceipt, FundingError> {
        settle_cash(self, command).await
    }

    /// PG 测试入口：退款转换。
    pub async fn test_convert_refund(
        &self,
        command: ConvertRefund,
    ) -> Result<RefundReceipt, FundingError> {
        convert_refund(self, command).await
    }

    /// PG 测试入口：先执行 `setup_sql`（本事务内的前置事实），再在同一事务内封闭未付 Sale。
    pub async fn test_cancel_unpaid(
        &self,
        setup_sql: &str,
        contract_id: Uuid,
        disposition_id: Uuid,
    ) -> Result<bool, FundingError> {
        let call = (setup_sql, CancelUnpaid { contract_id, disposition_id });
        run_serializable(self, &call, |tx, (setup, command)| {
            Box::pin(async move {
                run_setup(tx, setup).await?;
                cancel_unpaid_in_tx(tx, command).await
            })
        })
        .await
    }

    /// PG 测试入口：先执行 `setup_sql`（含该 intake 的起写事实），再在同一事务内建立阻断。
    pub async fn test_open_cash_return_block(
        &self,
        setup_sql: &str,
        block: &OpeningBlock,
    ) -> Result<Uuid, FundingError> {
        let call = (setup_sql, block);
        run_serializable(self, &call, |tx, (setup, block)| {
            Box::pin(async move {
                run_setup(tx, setup).await?;
                open_cash_return_block_in_tx(tx, block).await
            })
        })
        .await
    }

    /// PG 测试入口：先执行 `setup_sql`（含精确 Take 证明与消费行），再在同一事务内回补现金。
    /// `ids` 依次为合同、intake、消费、原 delivery。
    pub async fn test_return_cash(
        &self,
        setup_sql: &str,
        mutation: FundingMutation,
        ids: [Uuid; 4],
        leg: BlockedLeg,
        amount: i64,
        proof_digest: [u8; 32],
    ) -> Result<(), FundingError> {
        let call = (
            setup_sql,
            ReturnConsumptionFacts {
                mutation,
                contract_id: ids[0],
                intake_id: ids[1],
                consumption_id: ids[2],
                leg,
                original_delivery_id: ids[3],
                amount,
                proof_digest,
            },
        );
        run_serializable(self, &call, |tx, (setup, facts)| {
            Box::pin(async move {
                run_setup(tx, setup).await?;
                return_cash_in_tx(tx, facts).await
            })
        })
        .await
    }

    /// PG 测试入口：在事务内记录一条本金结论，返回是否新写入。
    pub async fn test_record_principal_fact(
        &self,
        contract_id: Uuid,
        carrier_obligation_id: Uuid,
        original_delivery_id: Uuid,
        conclusion: PrincipalConclusion,
        mode: FundingMode,
    ) -> Result<bool, FundingError> {
        let fact = PrincipalFact {
            contract_id,
            carrier_obligation_id,
            original_delivery_id,
            round: 1,
            conclusion,
            mode,
        };
        run_serializable(self, &fact, |tx, fact| {
            Box::pin(async move {
                run_setup(tx, "").await?;
                record_principal_fact_in_tx(tx, fact).await
            })
        })
        .await
    }

    /// PG 测试入口：先执行 `setup_sql`（含收据、分录与 funded 投影），再在同一事务内记录 earned。
    pub async fn test_write_fee_earned(
        &self,
        setup_sql: &str,
        contract_id: Uuid,
        receipt_source_id: Uuid,
        fee_entry_id: Uuid,
        quote: ListingQuote,
    ) -> Result<Option<Uuid>, FundingError> {
        let call = (setup_sql, contract_id, receipt_source_id, fee_entry_id, quote);
        run_serializable(self, &call, |tx, (setup, contract, receipt, entry, quote)| {
            Box::pin(async move {
                run_setup(tx, setup).await?;
                write_fee_earned_in_tx(tx, *contract, *receipt, *entry, *quote).await
            })
        })
        .await
    }
}
