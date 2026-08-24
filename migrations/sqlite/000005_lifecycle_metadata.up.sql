ALTER TABLE auctions ADD COLUMN preview_opened_at TEXT;
ALTER TABLE auctions ADD COLUMN preview_manual_open INTEGER NOT NULL DEFAULT 0
    CHECK (preview_manual_open IN (0, 1));
ALTER TABLE auctions ADD COLUMN preview_revision INTEGER NOT NULL DEFAULT 0
    CHECK (preview_revision >= 0);
ALTER TABLE auctions ADD COLUMN opened_at TEXT;
ALTER TABLE auctions ADD COLUMN final_phase_at TEXT;
ALTER TABLE auctions ADD COLUMN predecessor_auction_id BLOB REFERENCES auctions(id) ON DELETE RESTRICT;

CREATE UNIQUE INDEX auctions_one_successor_idx
    ON auctions (predecessor_auction_id)
    WHERE predecessor_auction_id IS NOT NULL;

CREATE TRIGGER auctions_lifecycle_shape_insert
AFTER INSERT ON auctions
BEGIN
    SELECT CASE
        WHEN NEW.preview_manual_open = 1 AND NEW.preview_opened_at IS NULL
        THEN RAISE(ABORT, 'manual preview requires preview_opened_at')
        WHEN NEW.opened_at IS NOT NULL AND NEW.preview_opened_at IS NULL
        THEN RAISE(ABORT, 'opened auction requires preview_opened_at')
        WHEN NEW.final_phase_at IS NOT NULL AND NEW.opened_at IS NULL
        THEN RAISE(ABORT, 'final auction requires opened_at')
    END;
END;

CREATE TRIGGER auctions_lifecycle_shape_update
AFTER UPDATE OF preview_opened_at, preview_manual_open, opened_at, final_phase_at ON auctions
BEGIN
    SELECT CASE
        WHEN NEW.preview_manual_open = 1 AND NEW.preview_opened_at IS NULL
        THEN RAISE(ABORT, 'manual preview requires preview_opened_at')
        WHEN NEW.opened_at IS NOT NULL AND NEW.preview_opened_at IS NULL
        THEN RAISE(ABORT, 'opened auction requires preview_opened_at')
        WHEN NEW.final_phase_at IS NOT NULL AND NEW.opened_at IS NULL
        THEN RAISE(ABORT, 'final auction requires opened_at')
    END;
END;
