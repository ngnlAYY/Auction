-- 旧 v1 schema 不允许 live/sold lot 缺少有效结束时间。先锁住写入并阻断已有非法
-- 投影，再追加命名约束；迁移不得猜测或回填业务截止时间。
LOCK TABLE public.lots IN SHARE ROW EXCLUSIVE MODE;

DO $$
BEGIN
    IF EXISTS (
        SELECT 1
        FROM public.lots
        WHERE status IN ('live', 'sold')
          AND effective_end_at IS NULL
    ) THEN
        RAISE EXCEPTION
            'lot effective-end invariant migration found a live or sold lot without effective_end_at'
            USING ERRCODE = '23514';
    END IF;
END
$$;

ALTER TABLE public.lots
    ADD CONSTRAINT lots_effective_end_shape_check CHECK (
        (status IN ('live', 'sold') AND effective_end_at IS NOT NULL)
        OR status IN ('pending', 'unsold', 'withdrawn')
    );
