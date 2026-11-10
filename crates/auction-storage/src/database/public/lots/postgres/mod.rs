//! PostgreSQL Public lot 读取适配器装配；列表、详情和行映射由子模块拥有，本模块只保留稳定导出。

// 列表与详情共用同一行投影和可见性条件，SQL 片段以字面量宏组合成静态查询。

/// 预览 lot 行：只在世界启用且拍卖处于 preview 时公开；调用方追加过滤条件。
macro_rules! preview_lot_sql {
    ($($tail:tt)+) => {
        concat!(
            "SELECT
                l.id,
                l.equipment_id,
                l.auction_id,
                l.category_number AS lot_number,
                l.status,
                l.start_price,
                submission.seller AS seller_name,
                e.external_equipment_id,
                e.name AS equipment_name,
                e.category AS equipment_category,
                e.attributes AS equipment_attributes
             FROM public.lots AS l
             JOIN public.auctions AS a ON a.id = l.auction_id
             JOIN public.worlds AS w ON w.id = a.world_id AND w.enabled
             JOIN public.equipment AS e ON e.id = l.equipment_id
             JOIN public.auction_submissions AS submission ON submission.id = l.submission_id
             WHERE a.status = 'preview' AND l.status = 'pending' AND ",
            $($tail)+
        )
    };
}

/// 正式 lot 行：进行中/截止阶段公开 live、sold、unsold，已结算只公开终态；调用方追加过滤条件。
macro_rules! live_lot_sql {
    ($($tail:tt)+) => {
        concat!(
            "SELECT
                l.id,
                l.equipment_id,
                l.auction_id,
                l.category_number AS lot_number,
                l.status,
                l.start_price,
                l.current_price,
                (l.leader_user_id IS NOT NULL)::boolean AS has_leader,
                l.material,
                (a.policy_snapshot ->> 'bid_increment_ppm')::bigint AS policy_bid_increment_ppm,
                (a.policy_snapshot ->> 'minimum_increment')::bigint AS policy_minimum_increment,
                leader.username AS leader_name,
                submission.seller AS seller_name,
                l.effective_end_at,
                l.state_version,
                e.external_equipment_id,
                e.name AS equipment_name,
                e.category AS equipment_category,
                e.attributes AS equipment_attributes
             FROM public.lots AS l
             JOIN public.auctions AS a ON a.id = l.auction_id
             JOIN public.worlds AS w
               ON w.id = a.world_id AND (w.enabled OR a.status = 'settled')
             JOIN public.equipment AS e ON e.id = l.equipment_id
             LEFT JOIN public.auction_submissions AS submission ON submission.id = l.submission_id
             LEFT JOIN public.users AS leader ON leader.id = l.leader_user_id
             WHERE ((a.status IN ('live', 'final', 'settling') AND l.status IN ('live', 'sold', 'unsold'))
                    OR (a.status = 'settled' AND l.status IN ('sold', 'unsold')))
               AND ",
            $($tail)+
        )
    };
}

mod detail;
mod list;
mod mapping;
mod selection;

pub(super) use detail::detail_postgres;
pub(super) use list::list_postgres;
