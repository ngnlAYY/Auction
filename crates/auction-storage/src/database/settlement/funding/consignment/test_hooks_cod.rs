//! `test-db` 下暴露给 PG 用例的收款侧入口；生产构建不含本模块。

use auction_application::settlement::{
    AttributionReceipt, CertifyCod, CertifyReceipt, FundingError, RegisterOpeningAttribution,
};
use auction_domain::consignment::funding::CodReceiptSplit;
use uuid::Uuid;

use super::certify_cod::certify_cod;
use super::opening_slice::{register_opening_attribution, rule};
use super::receipt::{RemotePayment, insert_receipt, receipt_key_id};
use super::serializable::run_serializable;
use crate::DatabaseStore;
use crate::database::settlement::funding::ledger::source;
use crate::database::settlement::funding::lock_funding_worlds;

impl DatabaseStore {
    /// PG 测试入口：COD 实收认证。
    pub async fn test_certify_cod(
        &self,
        command: CertifyCod,
    ) -> Result<CertifyReceipt, FundingError> {
        certify_cod(self, command).await
    }

    /// PG 测试入口：opening 份额归属登记。
    pub async fn test_register_opening_attribution(
        &self,
        command: RegisterOpeningAttribution,
    ) -> Result<AttributionReceipt, FundingError> {
        register_opening_attribution(self, command).await
    }

    /// PG 测试入口：在一个事务内为命令建立零额付款 source 并写收据；命令的 Sent MID 若没有对应绑定，
    /// 寄售收据的 INSERT…SELECT 零行，必须失败关闭并连同通用收据一起回滚。
    pub async fn test_insert_receipt(&self, command: CertifyCod) -> Result<(), FundingError> {
        run_serializable(self, &command, |tx, command| {
            Box::pin(async move {
                lock_funding_worlds(tx).await?;
                let split = CodReceiptSplit::new(command.gross, command.gross).map_err(rule)?;
                let key_id = receipt_key_id(&command.receipt);
                let digest = [7; 32];
                let source_id = source(
                    tx,
                    &command.receipt.uid,
                    "collected_payment",
                    command.payment_obligation_id,
                    0,
                    &digest,
                )
                .await?;
                let remote = RemotePayment {
                    evidence_id: Uuid::new_v4(),
                    reference: "unbound-observation".to_owned(),
                };
                insert_receipt(tx, command, source_id, key_id, &remote, split).await
            })
        })
        .await
    }
}
