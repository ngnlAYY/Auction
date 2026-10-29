-- 赛季早鸟场次允许同一世界同一本地周存在多个轮次。
-- 常规周轮次（slot_key 为 NULL）仍按 world/local_week 唯一；早鸟轮次按 world/slot_key 唯一，
-- 且 slot_key 必须与冻结排期来源中的早鸟身份同时出现。既有轮次全部保持常规周身份，不回填。
ALTER TABLE public.scheduled_rounds
    ADD COLUMN slot_key text;

ALTER TABLE public.scheduled_rounds
    ADD CONSTRAINT scheduled_rounds_slot_key_shape_check CHECK (
        slot_key IS NULL
        OR slot_key ~ '^early-bird:[0-9]{4}-[0-9]{2}-[0-9]{2}:[1-9][0-9]?$'
    ),
    ADD CONSTRAINT scheduled_rounds_slot_key_source_check CHECK (
        (slot_key IS NULL)
        = (schedule_snapshot IS NULL OR NOT (schedule_snapshot ? 'early_bird'))
    );

ALTER TABLE public.scheduled_rounds
    DROP CONSTRAINT scheduled_rounds_world_id_local_week_key;

CREATE UNIQUE INDEX scheduled_rounds_weekly_identity_key
    ON public.scheduled_rounds (world_id, local_week)
    WHERE slot_key IS NULL;

CREATE UNIQUE INDEX scheduled_rounds_slot_identity_key
    ON public.scheduled_rounds (world_id, slot_key)
    WHERE slot_key IS NOT NULL;
