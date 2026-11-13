-- 竞价警告渠道偏好从“MM 或 PM 二选一”改为集合：MM 与 PM 各自独立，可同时启用，也可都关闭。
-- owner: auction-domain（`LotWatchChannels` 闭集）、auction-storage（lot_watch 偏好与提醒事实）
--
-- 不变量：
--   1. 账号偏好闭集扩为 none / mm / pm / both；既有偏好值与版本不改写。none 表示标记只作
--      收藏、不产生提醒；both 表示同一被超事件在两个渠道各发一条。
--   2. 每条提醒来源与待定事实仍只属于一个渠道（mm 或 pm），两张表的 channel CHECK 不变。
--      唯一性由“每个 lot × 用户 × 领先周期一条”改为“每个 lot × 用户 × 领先周期 × 渠道一条”：
--      同一领先周期在同一渠道仍然最多一条，不会重发。
--   3. 既有来源、待定事实、投递、租约与审计行原样保留；本迁移不生成、不改写、不重新路由
--      任何提醒，偏好变更只影响之后的被超事件。
--
-- 等级：breaking。旧程序不认识 none / both：读取偏好与写提醒时闭集解析失败；旧程序写待定
-- 事实使用的 ON CONFLICT (lot_id, user_id, leadership_cycle) 也不再匹配任何唯一约束。
-- 须先停止旧 Public 与 Worker 再迁移，回退须按 RECOVERY.md 处理含 breaking 迁移的版本。
ALTER TABLE public.users
    DROP CONSTRAINT users_lot_watch_channel_check,
    ADD CONSTRAINT users_lot_watch_channel_check
        CHECK (lot_watch_channel IN ('none', 'mm', 'pm', 'both'));

ALTER TABLE public.lot_watch_notifications
    DROP CONSTRAINT lot_watch_notifications_lot_id_user_id_leadership_cycle_key,
    ADD CONSTRAINT lot_watch_notifications_cycle_channel_key
        UNIQUE (lot_id, user_id, leadership_cycle, channel);

ALTER TABLE public.lot_watch_pending_notifications
    DROP CONSTRAINT lot_watch_pending_notificatio_lot_id_user_id_leadership_cyc_key,
    ADD CONSTRAINT lot_watch_pending_notifications_cycle_channel_key
        UNIQUE (lot_id, user_id, leadership_cycle, channel);

INSERT INTO public.audit_records
    (id, actor_type, actor_id, action, entity_type, entity_id, result, source, metadata)
VALUES (
    gen_random_uuid(), 'system', 'migration-000105', 'system.migration', 'migration',
    '000105', 'success', 'system',
    jsonb_build_object(
        'summary', 'lot watch channel preference becomes a set of mm and pm',
        'compatibility', 'breaking')
);

INSERT INTO public.schema_migration_compatibility (version, level)
VALUES (105, 'breaking');
