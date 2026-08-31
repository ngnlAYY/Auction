-- Lite 与完整档共享同一编号语义：内部全局顺序不变，对外编号按装备分类独立递增。
ALTER TABLE lots ADD COLUMN category_key TEXT;
ALTER TABLE lots ADD COLUMN category_number INTEGER;

WITH ranked AS (
    SELECT
        lot.id,
        equipment.category AS category_key,
        row_number() OVER (
            PARTITION BY lot.auction_id, equipment.category
            ORDER BY lot.lot_number, lot.id
        ) AS category_number
    FROM lots AS lot
    INNER JOIN equipment ON equipment.id = lot.equipment_id
)
UPDATE lots
SET category_key = (SELECT ranked.category_key FROM ranked WHERE ranked.id = lots.id),
    category_number = (SELECT ranked.category_number FROM ranked WHERE ranked.id = lots.id);

CREATE UNIQUE INDEX lots_auction_category_number_idx
    ON lots (auction_id, category_key, category_number);
CREATE INDEX lots_auction_category_status_idx
    ON lots (auction_id, category_key, status, category_number);

CREATE TRIGGER lots_category_number_shape_insert
BEFORE INSERT ON lots
WHEN NEW.category_key IS NULL
  OR trim(NEW.category_key) = ''
  OR NEW.category_key <> trim(NEW.category_key)
  OR NEW.category_number IS NULL
  OR NEW.category_number <= 0
  OR NEW.category_key IS NOT (SELECT category FROM equipment WHERE id = NEW.equipment_id)
BEGIN
    SELECT RAISE(ABORT, 'lot category numbering is invalid');
END;

CREATE TRIGGER lots_category_number_shape_update
BEFORE UPDATE OF equipment_id, category_key, category_number ON lots
WHEN NEW.category_key IS NULL
  OR trim(NEW.category_key) = ''
  OR NEW.category_key <> trim(NEW.category_key)
  OR NEW.category_number IS NULL
  OR NEW.category_number <= 0
  OR NEW.category_key IS NOT (SELECT category FROM equipment WHERE id = NEW.equipment_id)
BEGIN
    SELECT RAISE(ABORT, 'lot category numbering is invalid');
END;

CREATE TRIGGER lots_category_numbering_immutable
BEFORE UPDATE OF auction_id, equipment_id, lot_number, category_key, category_number ON lots
WHEN NEW.auction_id IS NOT OLD.auction_id
  OR NEW.equipment_id IS NOT OLD.equipment_id
  OR NEW.lot_number IS NOT OLD.lot_number
  OR NEW.category_key IS NOT OLD.category_key
  OR NEW.category_number IS NOT OLD.category_number
BEGIN
    SELECT RAISE(ABORT, 'lot numbering identity is immutable');
END;

-- 触发一次不改变值的校验；历史数据缺少分类或编号时整份迁移失败关闭。
UPDATE lots
SET category_key = category_key,
    category_number = category_number;
