//! 寄售公开、本人有效状态、额度与写侧清退的共用 SQL 谓词。

/// 拼接带固定别名的静态 SQL。卖家 s、挂单 l、准入单例 c 由调用者连接。
macro_rules! listing_sql {
    // 按同一世界及数据库规范别名核对旧装备行。
    (equipment_alias, $world:literal, $eid:literal) => {
        concat!(
            "e.world_id = ", $world,
            " AND public.consignment_eid_alias(e.external_equipment_id) = ", $eid
        )
    };
    // 匹配当前挂单 l 的装备行、lot a 及不可变登记基线的差集。
    (unbaselined_lots) => {
        concat!(
            "FROM public.equipment e JOIN public.lots a ON a.equipment_id = e.id WHERE ",
            listing_sql!(equipment_alias, "l.world_id", "l.external_equipment_id"),
            " AND NOT EXISTS (
                SELECT 1 FROM public.consignment_listing_lot_baselines b
                WHERE b.listing_id = l.id AND b.lot_id = a.id
            )"
        )
    };
    // 基线外且尚未关联的 lot，供关闭和指定观察使用。
    (unassociated_lots) => {
        concat!(
            listing_sql!(unbaselined_lots),
            " AND NOT EXISTS (
                 SELECT 1 FROM public.consignment_listing_auction_conflicts hit
                 WHERE hit.listing_id = l.id AND hit.lot_id = a.id
              )"
        )
    };
    // 查询已提交 lot 的基线差集；不按路径、拍卖状态或时间筛选。
    (collision) => {
        concat!("EXISTS (SELECT 1 ", listing_sql!(unbaselined_lots), ")")
    };
    // 基于卖家别名 s 读取当前拉黑投影。
    (active_blacklist) => {
        "EXISTS (SELECT 1 FROM public.user_blacklist bl
                 WHERE bl.user_id = s.id AND bl.revoked_at IS NULL)"
    };
    // 基于卖家别名 s 计数追加历史，解禁后也不会回退。
    (blacklist_count) => {
        "(SELECT count(*) FROM public.user_blacklist_history bh WHERE bh.user_id = s.id)"
    };
    // 卖家当前账户状态。
    (seller_active) => {
        "s.status = 'active'"
    };
    // 卖家当前状态与当前拉黑事实。
    (seller_eligible) => {
        concat!("(", listing_sql!(seller_active), " AND NOT ", listing_sql!(active_blacklist), ")")
    };
    // 当前卖家资格与登记时冻结的拉黑历史共同决定旧单资格。
    (listing_seller_eligible) => {
        concat!(
            "(", listing_sql!(seller_eligible),
            " AND ", listing_sql!(blacklist_count), " = l.blacklist_count_at_register)"
        )
    };
    // 仅当前且准入中的代次有效；单例缺失时关闭资格。
    (admitting_epoch) => {
        "COALESCE(c.admitting AND l.service_epoch = c.enabled_epoch, false)"
    };
    // 当前态的截止等号即失效，时刻由调用者绑定。
    (unexpired, $as_of:literal) => {
        concat!("(l.expires_at > ", $as_of, "::timestamptz)")
    };
    // 尚未持久终结的挂单，匹配数据库部分索引。
    (stored_active) => {
        "l.state IN ('resolving', 'displayed', 'awaiting_deposit')"
    };
    // 已核验发布的持久状态。
    (stored_published) => {
        "l.state IN ('displayed', 'awaiting_deposit')"
    };
    // 为带别名 l 的挂单补齐卖家 s 与准入单例 c。
    (eligibility_joins) => {
        " JOIN public.users s ON s.id = l.seller_user_id
          LEFT JOIN public.consignment_lookup_control c ON c.id = 1 "
    };
    // 返回活动挂单最高优先级的强制原因码，持久终态及有效活动均为 NULL。
    (forced_reason, $as_of:literal) => {
        concat!(
            "(CASE WHEN NOT ", listing_sql!(stored_active), " THEN NULL::text
                   WHEN ", listing_sql!(collision), " THEN 'auction_lot_created'
                   WHEN NOT ", listing_sql!(listing_seller_eligible), " THEN 'seller_ineligible'
                   WHEN NOT ", listing_sql!(admitting_epoch), " THEN 'service_disabled'
                   WHEN NOT ", listing_sql!(unexpired, $as_of), " THEN CASE l.state
                       WHEN 'resolving' THEN 'lookup_deadline'
                       WHEN 'displayed' THEN 'display_expired'
                       WHEN 'awaiting_deposit' THEN 'deposit_expired' END
              END)"
        )
    };
    // 跨世界活动额度与同件有效占用共用的谓词。
    (effective_active, $as_of:literal) => {
        concat!(
            "(", listing_sql!(stored_active), " AND ", listing_sql!(unexpired, $as_of),
            " AND ", listing_sql!(forced_reason, $as_of), " IS NULL)"
        )
    };
    // 目录和详情共用的谓词，要求当前快照确属本挂单。
    (public_visible, $as_of:literal) => {
        concat!(
            "(", listing_sql!(stored_published), " AND ", listing_sql!(effective_active, $as_of),
            " AND EXISTS (SELECT 1 FROM public.consignment_snapshots snapshot
                          WHERE snapshot.listing_id = l.id AND snapshot.id = l.current_snapshot_id))"
        )
    };
}
pub(crate) use listing_sql;
