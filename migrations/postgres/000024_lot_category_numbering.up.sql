-- 对外拍品编码按装备分类独立编号；原 lot_number 继续作为场次内不可变全局顺序。
ALTER TABLE public.lots
    ADD COLUMN category_key text,
    ADD COLUMN category_number integer;

WITH ranked AS (
    SELECT
        lot.id,
        equipment.category AS category_key,
        row_number() OVER (
            PARTITION BY lot.auction_id, equipment.category
            ORDER BY lot.lot_number, lot.id
        )::integer AS category_number
    FROM public.lots AS lot
    INNER JOIN public.equipment AS equipment ON equipment.id = lot.equipment_id
)
UPDATE public.lots AS lot
SET category_key = ranked.category_key,
    category_number = ranked.category_number
FROM ranked
WHERE ranked.id = lot.id;

ALTER TABLE public.lots
    ALTER COLUMN category_key SET NOT NULL,
    ALTER COLUMN category_number SET NOT NULL,
    ADD CONSTRAINT lots_category_key_shape_check
        CHECK (category_key = btrim(category_key) AND category_key <> ''),
    ADD CONSTRAINT lots_category_number_positive_check
        CHECK (category_number > 0),
    ADD CONSTRAINT lots_auction_category_number_key
        UNIQUE (auction_id, category_key, category_number);

CREATE INDEX lots_auction_category_status_idx
    ON public.lots (auction_id, category_key, status, category_number);

-- 已经开始发布的论坛主题必须继续解释原全局编号；尚未领取的主题和后续新主题使用分类编号。
ALTER TABLE public.auction_forum_sync
    ADD COLUMN numbering_scheme text;

UPDATE public.auction_forum_sync
SET numbering_scheme = CASE
    WHEN status = 'pending'
         AND thread_id IS NULL
         AND thread_url IS NULL
         AND title IS NULL
         AND body IS NULL
        THEN 'category_v1'
    ELSE 'legacy_global'
END;

ALTER TABLE public.auction_forum_sync
    ALTER COLUMN numbering_scheme SET DEFAULT 'category_v1',
    ALTER COLUMN numbering_scheme SET NOT NULL,
    ADD CONSTRAINT auction_forum_sync_numbering_scheme_check
        CHECK (numbering_scheme IN ('legacy_global', 'category_v1'));

CREATE FUNCTION public.enforce_lot_category_numbering()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
    stored_category text;
BEGIN
    SELECT category
    INTO stored_category
    FROM public.equipment
    WHERE id = NEW.equipment_id;

    IF stored_category IS NULL OR NEW.category_key IS DISTINCT FROM stored_category THEN
        RAISE EXCEPTION 'lot category key must match equipment category'
            USING ERRCODE = '23514';
    END IF;

    IF TG_OP = 'UPDATE'
       AND (
           NEW.auction_id IS DISTINCT FROM OLD.auction_id
           OR NEW.equipment_id IS DISTINCT FROM OLD.equipment_id
           OR NEW.lot_number IS DISTINCT FROM OLD.lot_number
           OR NEW.category_key IS DISTINCT FROM OLD.category_key
           OR NEW.category_number IS DISTINCT FROM OLD.category_number
       ) THEN
        RAISE EXCEPTION 'lot numbering identity is immutable'
            USING ERRCODE = '55000';
    END IF;

    RETURN NEW;
END;
$$;

CREATE TRIGGER lots_category_numbering_guard
BEFORE INSERT OR UPDATE OF auction_id, equipment_id, lot_number, category_key, category_number
ON public.lots
FOR EACH ROW EXECUTE FUNCTION public.enforce_lot_category_numbering();
