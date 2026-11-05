-- storage owner：W12.4 公开成交价格历史（`PublicPriceHistoryStore`）的已成交 lot 截止时间候选索引。
--
-- 目的：价格历史的可见集合一直是“已结算场次里 status='sold' 且存在买家与结算记录”的 lot，
-- 游标页按 `l.effective_end_at DESC, a.sequence DESC, l.lot_number, l.id` 取顶部若干行，但
-- `public.lots` 没有任何以 `effective_end_at` 为前导的索引，只能顺序扫描全部 lot 再排序。
-- 该索引让游标页退化为按截止时间倒序读少量堆行，编号页（全量统计）仍按既有计划执行。
--
-- 不变量：索引只是候选入口。可见集合、排序、总数、中位价与筛选仍由
-- `a.status = 'settled'`、`l.status = 'sold'`、`l.leader_user_id IS NOT NULL`、
-- lot/auction 结算记录存在以及全部过滤条件决定；金额、资格、锁序、审计、事件与冻结策略均不变。
-- 谓词与查询里恒定的条件一致（`status = 'sold' AND leader_user_id IS NOT NULL`），
-- `INCLUDE` 只补充游标页需要的非搜索列，不改变任何唯一性或写入要求。
--
-- compatible：纯新增普通索引，旧程序不读取、不依赖，也不会因它拒绝任何既有写入；
-- 索引在停机门禁下随其它迁移同事务创建，不需要旧角色先停止。
CREATE INDEX lots_sold_history_effective_end_idx
    ON public.lots (effective_end_at DESC, id)
    INCLUDE (auction_id, current_price, lot_number)
    WHERE status = 'sold' AND leader_user_id IS NOT NULL;

INSERT INTO public.schema_migration_compatibility (version, level)
VALUES (84, 'compatible');
