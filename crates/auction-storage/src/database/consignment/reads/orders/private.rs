//! 本人订单：列表与详情。只按会话用户过滤，非本人与不存在同样返回空（不变量 7）。
//!
//! 角色由行本身决定（买家列是本人则为买方，否则为卖方）；金额在 `OwnedOrder::new` 按角色省略。

use auction_application::consignment::{
    ConsignmentError, ConsignmentFuture, ListingReadPage, OrderQuery, OrderRole, OwnedOrder,
    OwnedOrderRead,
};
use auction_domain::UserId;
use sqlx::{PgConnection, Postgres, QueryBuilder};
use uuid::Uuid;

use super::{LoadedOrder, load_orders};
use crate::database::DatabaseStore;
use crate::database::consignment::reads::{snapshot, storage_error, unavailable};

/// 带状态筛选时最多扫描的订单数；超过则失败关闭，不在不完整的集合上给出“精确总数”。
const STATE_SCAN_LIMIT: i64 = 2_000;

/// 候选合同的筛选范围。
pub(super) struct Scope {
    pub(super) user_id: Option<Uuid>,
    pub(super) role: Option<OrderRole>,
    pub(super) world: Option<auction_application::consignment::ListingWorld>,
}

fn candidates(select: &str, scope: &Scope) -> QueryBuilder<Postgres> {
    let mut sql = QueryBuilder::new(select);
    sql.push(
        " FROM public.consignment_contracts c
          LEFT JOIN public.consignment_sales s ON s.contract_id = c.contract_id WHERE true",
    );
    if let Some(user_id) = scope.user_id {
        match scope.role {
            Some(OrderRole::Buyer) => {
                sql.push(" AND s.buyer_user_id = ").push_bind(user_id);
            }
            Some(OrderRole::Seller) => {
                sql.push(" AND c.seller_user_id = ").push_bind(user_id);
            }
            None => {
                sql.push(" AND (s.buyer_user_id = ")
                    .push_bind(user_id)
                    .push(" OR c.seller_user_id = ")
                    .push_bind(user_id)
                    .push(")");
            }
        }
    }
    if let Some(world) = scope.world {
        sql.push(" AND c.world_id = ").push_bind(world.as_str());
    }
    sql
}

const ORDERING: &str = " ORDER BY c.created_at DESC, c.contract_id DESC";

/// 分页加载订单。无状态筛选时由 SQL 分页并给出精确总数；有状态筛选时扫描上限内全部候选，
/// 在投影后的订单态上过滤，因为订单态不是存储列。
pub(super) async fn page_orders(
    conn: &mut PgConnection,
    scope: &Scope,
    state: Option<auction_domain::consignment::OrderState>,
    window: auction_application::consignment::ListingPageWindow,
) -> Result<(Vec<LoadedOrder>, u64), ConsignmentError> {
    if let Some(state) = state {
        let mut sql = candidates("SELECT c.contract_id", scope);
        sql.push(ORDERING).push(" LIMIT ").push_bind(STATE_SCAN_LIMIT + 1);
        let ids: Vec<Uuid> =
            sql.build_query_scalar().fetch_all(&mut *conn).await.map_err(storage_error)?;
        if i64::try_from(ids.len()).map_err(unavailable)? > STATE_SCAN_LIMIT {
            return Err(ConsignmentError::Unavailable);
        }
        let mut matched = load_orders(conn, &ids)
            .await
            .map_err(storage_error)?
            .into_iter()
            .filter(|order| order.core.stages.order_state == state)
            .collect::<Vec<_>>();
        let total = matched.len() as u64;
        let start = usize::try_from(window.offset()).map_err(unavailable)?;
        let size = usize::try_from(window.page_size()).map_err(unavailable)?;
        let page = matched.drain(..).skip(start).take(size).collect();
        return Ok((page, total));
    }
    let total: i64 = candidates("SELECT count(*)", scope)
        .build_query_scalar()
        .fetch_one(&mut *conn)
        .await
        .map_err(storage_error)?;
    let mut sql = candidates("SELECT c.contract_id", scope);
    sql.push(ORDERING)
        .push(" LIMIT ")
        .push_bind(i64::from(window.page_size()))
        .push(" OFFSET ")
        .push_bind(i64::from(window.offset()));
    let ids: Vec<Uuid> =
        sql.build_query_scalar().fetch_all(&mut *conn).await.map_err(storage_error)?;
    let page = load_orders(conn, &ids).await.map_err(storage_error)?;
    Ok((page, total.cast_unsigned()))
}

fn owned(order: LoadedOrder, user_id: Uuid) -> OwnedOrder {
    let role =
        if order.buyer_user_id == Some(user_id) { OrderRole::Buyer } else { OrderRole::Seller };
    OwnedOrder::new(order.core, role)
}

impl OwnedOrderRead for DatabaseStore {
    fn list_owned_orders(
        &self,
        user_id: UserId,
        query: OrderQuery,
    ) -> ConsignmentFuture<'_, ListingReadPage<OwnedOrder>> {
        Box::pin(async move {
            let user = user_id.as_uuid();
            let (mut tx, _) = snapshot(self, false).await?;
            let scope = Scope { user_id: Some(user), role: query.role, world: query.world };
            let (orders, total) = page_orders(&mut tx, &scope, query.state, query.window).await?;
            tx.commit().await.map_err(storage_error)?;
            Ok(ListingReadPage {
                items: orders.into_iter().map(|order| owned(order, user)).collect(),
                total,
                window: query.window,
            })
        })
    }

    fn owned_order(
        &self,
        user_id: UserId,
        contract_id: Uuid,
    ) -> ConsignmentFuture<'_, Option<OwnedOrder>> {
        Box::pin(async move {
            let user = user_id.as_uuid();
            let (mut tx, _) = snapshot(self, false).await?;
            let mut orders = load_orders(&mut tx, &[contract_id]).await.map_err(storage_error)?;
            tx.commit().await.map_err(storage_error)?;
            // 非本人与不存在同形：只有买家或卖家本人的订单才返回。
            Ok(orders
                .pop()
                .filter(|order| order.buyer_user_id == Some(user) || order.seller_user_id == user)
                .map(|order| owned(order, user)))
        })
    }
}
