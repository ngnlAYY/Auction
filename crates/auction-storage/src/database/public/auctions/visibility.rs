//! Public 拍卖投影与预览可见性的共享 SQL 片段；拍卖列表、当前场次、realtime 状态、倒计时与
//! lot 读取共用同一展示模式和门禁规则，避免各查询各自复制后漂移。

/// Public 拍卖投影列：标识、排期、计划截止、预览修订、展示模式与竞价开放。`a` 为拍卖别名，
/// `$lot` 为按 `a.id` 分组的同场 lot 别名；预览场以数据库时钟判定，到开拍时刻后显示 opening。
macro_rules! public_auction_projection_sql {
    ($lot:literal) => {
        concat!(
            "a.id,
            a.world_id,
            a.sequence,
            a.status,
            a.scheduled_open_at,
            MAX(",
            $lot,
            ".effective_end_at) FILTER (WHERE ",
            $lot,
            ".status IN ('live', 'sold', 'unsold')) AS scheduled_ends_at,
            a.preview_revision,
            CASE
                WHEN a.status = 'preview' AND CURRENT_TIMESTAMP < a.scheduled_open_at THEN 'preview'
                WHEN a.status = 'preview' THEN 'opening'
                ELSE 'live'
            END AS presentation_mode,
            (a.status IN ('live', 'final')) AS bidding_open"
        )
    };
}
pub(crate) use public_auction_projection_sql;

/// 预览场只在同世界没有进行中场次时公开，后继预览不能绕过当前活动场次；调用方须已把
/// `a.status` 限定在公开状态集合内。
macro_rules! preview_visibility_sql {
    () => {
        "(a.status <> 'preview'
            OR NOT EXISTS (
                SELECT 1 FROM public.auctions AS active
                WHERE active.world_id = a.world_id
                  AND active.status IN ('live', 'final', 'settling')
            ))"
    };
}
pub(crate) use preview_visibility_sql;
