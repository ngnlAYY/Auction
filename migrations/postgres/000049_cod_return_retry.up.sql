-- 未付款 COD 被退回后的重发与三次退回收口。
-- 每次识别退回累加 cod_return_count；第 1、2 次重发 COD，第 3 次把装备退回卖家。
ALTER TABLE public.settlement_recoveries
    ADD COLUMN cod_return_count integer NOT NULL DEFAULT 0
        CHECK (cod_return_count BETWEEN 0 AND 3);

ALTER TABLE public.settlement_recoveries
    DROP CONSTRAINT settlement_recoveries_resolution_check;

ALTER TABLE public.settlement_recoveries
    ADD CONSTRAINT settlement_recoveries_resolution_check CHECK (
        resolution IS NULL
        OR resolution IN (
            'reclaimed',
            'cancelled',
            'equipment_recovered',
            'returned_after_three_cod_returns'
        )
    );
