//! Admin 订单：列表、详情与证据分页。Admin 同时看到买卖双方，金额不按角色省略。
//!
//! 读取不受寄售开关影响（D9 §7：关闭截止新承诺，不隐藏已有订单）；证据只含 ID、阶段与摘要。

use auction_application::consignment::{
    AdminOrder, AdminOrderQuery, AdminOrderRead, ConsignmentFuture, ListingPageWindow,
    ListingReadPage, OrderEvidence,
};
use auction_domain::UserId;
use uuid::Uuid;

use super::evidence::evidence_future;
use super::private::{Scope, page_orders};
use super::{LoadedOrder, load_orders};
use crate::database::DatabaseStore;
use crate::database::consignment::reads::{snapshot, storage_error};

fn admin(order: LoadedOrder) -> AdminOrder {
    AdminOrder {
        core: order.core,
        buyer_user_id: order.buyer_user_id.map(UserId::from_uuid),
        buyer_name: order.buyer_name,
        seller_user_id: UserId::from_uuid(order.seller_user_id),
        seller_name: order.seller_name,
    }
}

impl AdminOrderRead for DatabaseStore {
    fn list_admin_orders(
        &self,
        query: AdminOrderQuery,
    ) -> ConsignmentFuture<'_, ListingReadPage<AdminOrder>> {
        Box::pin(async move {
            let (mut tx, _) = snapshot(self, false).await?;
            let scope = Scope { user_id: None, role: None, world: query.world };
            let (orders, total) = page_orders(&mut tx, &scope, query.state, query.window).await?;
            tx.commit().await.map_err(storage_error)?;
            Ok(ListingReadPage {
                items: orders.into_iter().map(admin).collect(),
                total,
                window: query.window,
            })
        })
    }

    fn admin_order(&self, contract_id: Uuid) -> ConsignmentFuture<'_, Option<AdminOrder>> {
        Box::pin(async move {
            let (mut tx, _) = snapshot(self, false).await?;
            let mut orders = load_orders(&mut tx, &[contract_id]).await.map_err(storage_error)?;
            tx.commit().await.map_err(storage_error)?;
            Ok(orders.pop().map(admin))
        })
    }

    fn order_evidence(
        &self,
        contract_id: Uuid,
        window: ListingPageWindow,
    ) -> ConsignmentFuture<'_, Option<ListingReadPage<OrderEvidence>>> {
        evidence_future(self, contract_id, window)
    }
}
