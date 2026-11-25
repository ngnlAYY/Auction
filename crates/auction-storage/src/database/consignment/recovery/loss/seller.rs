//! 卖家本人损失回执读端口的存储实现。
//!
//! 只读、不取锁、不写任何表；按卖家过滤，非本人与不存在同为 `None`（不变量 7）。视图本身不含私有
//! 说明、证据或金额，未知状态码说明库内数据损坏，失败关闭为不可用。

use auction_application::consignment::{
    ConsignmentError, ConsignmentFuture, LossCaseState, SellerLossCaseRead, SellerLossCaseRecord,
};
use auction_domain::UserId;
use uuid::Uuid;

use super::reads::seller_loss_case;
use crate::database::DatabaseStore;
use crate::database::consignment::commands::errors::WriteError;

impl SellerLossCaseRead for DatabaseStore {
    fn seller_loss_case(
        &self,
        seller_user_id: UserId,
        position_id: Uuid,
    ) -> ConsignmentFuture<'_, Option<SellerLossCaseRecord>> {
        Box::pin(async move {
            let view = seller_loss_case(self.pool(), seller_user_id.as_uuid(), position_id)
                .await
                .map_err(WriteError::into_application)?;
            view.map(|view| {
                Ok(SellerLossCaseRecord {
                    position_id: view.position_id,
                    loss_case_id: view.loss_case_id,
                    state: LossCaseState::parse(&view.state)
                        .ok_or(ConsignmentError::Unavailable)?,
                    recorded_at: view.recorded_at,
                    receipt_id: view.receipt_id,
                })
            })
            .transpose()
        })
    }
}
