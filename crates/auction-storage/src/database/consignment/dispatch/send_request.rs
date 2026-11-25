//! 发送请求读口：从已衔接的轮投递构造发送所需的冻结事实，供 Worker 的派发传输适配器使用。
//!
//! 保护不变量 4、6：只读，不写任何行、不发许可；收件人、主题、模板、正文摘要、附件装备、Credits
//! 与 COD 全部取轮与投递上冻结的内容，不按当前用户名或当前挂单重算。轮上冻结身份与投递载荷的身份
//! 摘要不一致、投递未衔接（履约行不指向它）或已不在 `claimed` / `sending` 时返回 `None`，
//! 调用方不发送。正文文本不入库：传输按冻结模板与本读口给出的事实重新渲染，并以冻结的正文摘要
//! 核对，不符即不发。成交冻结的资金 UID 与记账方式随请求给出（Withdrawal 合同没有成交，两者为空），
//! 传输据此要求现金腿的发信会话认证 UID 等于冻结 UID，并决定是否取同会话现金观察。

use auction_application::consignment::DispatchSendRequest;
use auction_domain::consignment::FundingAccountingMode;
use serde_json::Value;
use sqlx::Row;
use uuid::Uuid;

use super::first_dispatch::{payload_equipment_ids, payload_matches};
use super::round::load_by_delivery;
use crate::database::DatabaseStore;
use crate::database::consignment::commands::errors::WriteError;

impl DatabaseStore {
    /// 读取轮投递的发送请求（见模块说明）。
    pub(super) async fn load_dispatch_send_request(
        &self,
        delivery_id: Uuid,
    ) -> Result<Option<DispatchSendRequest>, WriteError> {
        // 同一只读事务里读轮、投递与装备名，得到一致快照。
        let mut transaction = self.pool().begin_with("BEGIN READ ONLY").await?;
        let Some(round) = load_by_delivery(&mut transaction, delivery_id).await? else {
            return Ok(None);
        };
        // 装备名取挂单登记时的安全快照（与购买冻结首轮内容时同一来源）。
        let row = sqlx::query(
            "SELECT delivery.recipient, delivery.payload,
                    (SELECT snapshot.name FROM public.consignment_snapshots AS snapshot
                     WHERE snapshot.listing_id = contract.listing_id
                     ORDER BY snapshot.generation DESC LIMIT 1) AS item_name,
                    sale.funding_uid, sale.funding_mode
             FROM public.deliveries AS delivery
             JOIN public.settlement_obligation_fulfillments AS fulfillment
               ON fulfillment.obligation_id = $2 AND fulfillment.delivery_id = delivery.id
             JOIN public.consignment_contracts AS contract ON contract.contract_id = $3
             LEFT JOIN public.consignment_sales AS sale ON sale.contract_id = contract.contract_id
             WHERE delivery.id = $1 AND delivery.status IN ('claimed', 'sending')
               AND delivery.origin_kind = 'settlement' AND delivery.origin_id = $2::text",
        )
        .bind(delivery_id)
        .bind(round.key.obligation_id)
        .bind(round.key.contract_id)
        .fetch_optional(&mut *transaction)
        .await?;
        transaction.commit().await?;
        let Some(row) = row else {
            return Ok(None);
        };
        let recipient: String = row.try_get("recipient")?;
        let payload: Value = row.try_get("payload")?;
        let item_name: Option<String> = row.try_get("item_name")?;
        let funding_uid: Option<String> = row.try_get("funding_uid")?;
        let funding_mode = match row.try_get::<Option<&str>, _>("funding_mode")? {
            None => None,
            Some("strict_cash") => Some(FundingAccountingMode::StrictCash),
            Some("record_only") => Some(FundingAccountingMode::RecordOnly),
            // 未知记账方式是持久数据损坏：失败关闭，不按任何一种方式发送（不变量 6）。
            Some(_) => return Err(WriteError::invariant("consignment_funding_mode")),
        };
        let frozen = payload_equipment_ids(&payload)
            .map(|equipment_ids| round.frozen(equipment_ids))
            .transpose()?
            .filter(|frozen| payload_matches(&payload, frozen));
        let (Some(frozen), Some(item_name)) = (frozen, item_name) else {
            tracing::warn!(
                code = "dispatch_send_request_mismatch",
                "寄售轮投递的冻结事实不一致，不发送"
            );
            return Ok(None);
        };
        Ok(Some(DispatchSendRequest {
            delivery_id,
            key: round.key,
            recipient,
            frozen,
            item_name,
            funding_uid,
            funding_mode,
        }))
    }
}
