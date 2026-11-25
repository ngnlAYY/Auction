//! v2 读查询：与 v1 同一有效态 FROM 链，不带可表达性过滤，另连 position、有效止售与撤回意愿。

use auction_application::consignment::PrivateListingQuery;
use auction_domain::UserId;
use auction_time::Timestamp;
use sqlx::{Postgres, QueryBuilder};
use uuid::Uuid;

use crate::database::consignment::reads::page_order;
use crate::database::consignment::reads::private::{
    ADMIN_COLUMNS, ADMIN_JOIN, push_listing_from, seller_filter,
};
use crate::database::consignment::reads::projection::{EQUIPMENT_JOIN, PRIVATE_COLUMNS, WORK_JOIN};

const CUSTODY_COLUMNS: &str = ", pos.id AS position_id, pos.state AS position_state,
    pos.version AS position_version, pos.current_generation AS position_generation,
    hold.id AS hold_id, hold.occurred_at AS hold_set_at,
    EXISTS (SELECT 1 FROM public.consignment_custody_controls intent
            JOIN public.consignment_custody_positions intent_position
                ON intent_position.id = intent.position_id
            WHERE intent_position.listing_id = l.id AND intent.kind = 'withdrawal_intent')
        AS withdrawal_intent,
    (SELECT lc.case_id FROM public.consignment_loss_cases lc
     WHERE lc.position_id = pos.id AND lc.seller_user_id = l.seller_user_id) AS loss_case_id";

// 一个挂单只有一个 position；仍按最新排序取一行，避免意外的多行放大页内行数。
// 有效止售：未被任何 hold_released 指向的 hold_set（`c4_control_predecessor_uq` 保证至多一个后继）。
const CUSTODY_JOIN: &str = " LEFT JOIN LATERAL (
    SELECT cp.id, cp.state, cp.version, cp.current_generation
    FROM public.consignment_custody_positions cp WHERE cp.listing_id = l.id
    ORDER BY cp.updated_at DESC, cp.id DESC LIMIT 1
) pos ON true
LEFT JOIN LATERAL (
    SELECT c.id, c.occurred_at FROM public.consignment_custody_controls c
    JOIN public.consignment_custody_positions hp ON hp.id = c.position_id
    WHERE hp.listing_id = l.id AND c.kind = 'hold_set'
        AND NOT EXISTS (SELECT 1 FROM public.consignment_custody_controls r
                        WHERE r.predecessor_id = c.id)
    ORDER BY c.occurred_at DESC, c.id DESC LIMIT 1
) hold ON true";

fn listing_query(as_of: Timestamp, page_rows: bool, admin: bool) -> QueryBuilder<Postgres> {
    let mut sql = QueryBuilder::new(if page_rows { PRIVATE_COLUMNS } else { "SELECT count(*)" });
    if page_rows {
        sql.push(CUSTODY_COLUMNS);
        if admin {
            sql.push(ADMIN_COLUMNS);
        }
    }
    push_listing_from(&mut sql, as_of);
    if page_rows {
        sql.push(EQUIPMENT_JOIN).push(WORK_JOIN).push(CUSTODY_JOIN);
        if admin {
            sql.push(ADMIN_JOIN);
        }
    }
    sql
}

/// 行查询与 `count(*)` 共用同一组筛选；`state` 取有效态，与 v1 的筛选口径一致。
pub(super) fn list_query(
    as_of: Timestamp,
    query: PrivateListingQuery,
    seller: Option<UserId>,
    admin: bool,
    page_rows: bool,
) -> QueryBuilder<Postgres> {
    let mut sql = listing_query(as_of, page_rows, admin);
    sql.push(" WHERE true");
    seller_filter(&mut sql, seller);
    if let Some(world) = query.world {
        sql.push(" AND l.world_id = ").push_bind(world.as_str());
    }
    if let Some(mode) = query.mode {
        sql.push(" AND l.mode = ").push_bind(mode.as_str());
    }
    if let Some(state) = query.state {
        sql.push(
            " AND COALESCE(public.consignment_reason_state(effective.forced_reason), l.state) = ",
        )
        .push_bind(state.as_str());
    }
    if page_rows {
        page_order(&mut sql, query.window);
    }
    sql
}

pub(super) fn detail_query(
    as_of: Timestamp,
    listing_id: Uuid,
    seller: Option<UserId>,
    admin: bool,
) -> QueryBuilder<Postgres> {
    let mut sql = listing_query(as_of, true, admin);
    sql.push(" WHERE true");
    seller_filter(&mut sql, seller);
    sql.push(" AND l.id = ").push_bind(listing_id);
    sql
}
