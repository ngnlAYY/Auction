-- scheduling owner：赛季前场次（异世界赛季开始日关服前的最后一场）使用早鸟场次序号 0，
-- 轮次身份为 `early-bird:YYYY-MM-DD:0`。只放宽时段键形状，既有轮次与排期快照保持原样。
-- 旧程序不会生成序号 0；新程序在未执行本迁移的库上插入赛季前轮次会被 CHECK 拒绝（失败关闭）。
ALTER TABLE public.scheduled_rounds
    DROP CONSTRAINT scheduled_rounds_slot_key_shape_check,
    ADD CONSTRAINT scheduled_rounds_slot_key_shape_check CHECK (
        slot_key IS NULL
        OR slot_key ~ '^early-bird:[0-9]{4}-[0-9]{2}-[0-9]{2}:(0|[1-9][0-9]?)$'
    );
