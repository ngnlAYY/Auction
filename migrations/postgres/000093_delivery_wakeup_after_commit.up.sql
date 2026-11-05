-- W12.8：把投递唤醒的 `pg_notify` 移出任何业务事务。
--
-- 背景：迁移 `000059` 用 `delivery_ready_after_insert` 触发器在插入 `lot_watch` 的 `pending`
-- 投递的同一事务内执行 `pg_notify('auction_delivery_ready')`。PostgreSQL 在提交含 NOTIFY 的
-- 事务时持有一把全局锁直到提交完成，以保证通知按提交顺序入队；因此每一笔插入被超提醒投递
-- 的事务都把提交串行到同一把锁上。迁移 `000092` 已把提醒入队移出出价 Serializable 事务，
-- 但提交后 outbox 的入队事务仍带这条触发器。
--
-- 不变量：
--   1. 通知只是“有新可发送投递”的提示，不携带任务，不代替领取。持久任务、租约、
--      fencing 与状态机仍由 `deliveries` 的 claim/发送路径决定。
--   2. 唤醒改由完成插入的进程在事务提交之后单独发送：调用 `public.signal_delivery_ready()`
--      的语句是独立自动提交事务，不延长投递入队事务的提交，也不持有任何业务行锁。
--   3. 通知按提交顺序可见：通知只能在对应投递已提交后发出；提交成功但进程在发送通知前
--      崩溃时，Worker 的轮询扫描仍是兜底，不会丢失投递也不会重复发送。全局提交顺序不再由
--      NOTIFY 队列提供；通知不携带载荷，消费端只做合并唤醒，领取顺序仍由 claim 查询的
--      优先级/created_at/租约决定。
--   4. 频道名与空载荷保持不变，LISTEN 端无需改动；旧的触发器函数只被该触发器使用，
--      一并删除，不留第二套唤醒路径。
-- 等级：breaking。本迁移删除既有触发器并改变唤醒发送位置；旧程序不再收到触发器通知，
--   只能按轮询扫描领取投递，属于迁移规则中的“修改/删除既有触发器”。停机门禁下同版
--   切换后新程序显式调用 `signal_delivery_ready()`。

DROP TRIGGER IF EXISTS delivery_ready_after_insert ON public.deliveries;
DROP FUNCTION IF EXISTS public.notify_delivery_ready();

-- 只发送固定频道与空载荷；由写入进程在事务提交后调用。
CREATE FUNCTION public.signal_delivery_ready() RETURNS void
LANGUAGE sql
VOLATILE
AS $$
    SELECT pg_notify('auction_delivery_ready', '')
$$;

-- 只允许运行时角色主动发送唤醒；读取与领取权限沿用既有授权面。
REVOKE ALL ON FUNCTION public.signal_delivery_ready() FROM PUBLIC;
GRANT EXECUTE ON FUNCTION public.signal_delivery_ready() TO auction_runtime;

INSERT INTO public.audit_records
    (id, actor_type, actor_id, action, entity_type, entity_id, result, source, metadata)
VALUES (
    gen_random_uuid(), 'system', 'migration-000093', 'system.migration', 'migration',
    '000093', 'success', 'system',
    jsonb_build_object(
        'summary', 'delivery wakeup published after commit instead of in-transaction trigger',
        'compatibility', 'breaking')
);

INSERT INTO public.schema_migration_compatibility (version, level)
VALUES (93, 'breaking');
