-- storage owner：Public 竞价日志（`crates/auction-storage/src/database/public/bid_log/`）按新建代理命令
-- 查找被排位击败的代理授权时使用的候选索引。
--
-- 目的：代理对代理竞争只给胜者追加一条出价事实，落败授权以 `exhausted`/`ranked_below` 转换记录在
-- 同一命令（`origin_command_id`）下。竞价日志对本页每条新建代理事实按该命令查这条转换；
-- `bid_proxy_authorization_transitions` 只有 `(authorization_id, transition_number)` 主键，没有以命令为
-- 前导的索引，逐行查找会顺序扫描整张只追加的转换表，代价随历史增长并被页大小放大。部分索引只收录
-- 排位落败的转换，把每行查找变为一次索引探测。
--
-- 不变量：索引只是候选入口。落败授权的判定、可见集合、排序、分页、总数与金额仍由竞价日志查询与
-- 转换事实决定；不改变转换写入、锁序、审计、事件或冻结策略。谓词与查询里恒定的条件一致
-- （`transition_kind = 'exhausted' AND reason = 'ranked_below'`）。
--
-- compatible：纯新增部分索引，旧程序不读取、不依赖，也不会因它拒绝任何既有写入；
-- 索引在停机门禁下随其它迁移同事务创建，不需要旧角色先停止。
CREATE INDEX bid_proxy_transitions_ranked_below_origin_idx
    ON public.bid_proxy_authorization_transitions (origin_command_id)
    WHERE transition_kind = 'exhausted' AND reason = 'ranked_below';

INSERT INTO public.schema_migration_compatibility (version, level)
VALUES (109, 'compatible');
