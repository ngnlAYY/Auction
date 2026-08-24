ALTER TABLE auctions ADD COLUMN cancelled_at TEXT;
ALTER TABLE auctions ADD COLUMN cancelled_reason TEXT;
ALTER TABLE auctions ADD COLUMN cancelled_by TEXT;

CREATE TRIGGER auctions_cancellation_shape_insert
AFTER INSERT ON auctions
BEGIN
    SELECT CASE
        WHEN NEW.status = 'cancelled'
             AND (NEW.cancelled_at IS NULL OR trim(COALESCE(NEW.cancelled_reason, '')) = ''
                  OR trim(COALESCE(NEW.cancelled_by, '')) = '')
        THEN RAISE(ABORT, 'cancelled auction metadata is required')
        WHEN NEW.status <> 'cancelled'
             AND (NEW.cancelled_at IS NOT NULL OR NEW.cancelled_reason IS NOT NULL
                  OR NEW.cancelled_by IS NOT NULL)
        THEN RAISE(ABORT, 'non-cancelled auction cannot have cancellation metadata')
    END;
END;

CREATE TRIGGER auctions_cancellation_shape_update
AFTER UPDATE OF status, cancelled_at, cancelled_reason, cancelled_by ON auctions
BEGIN
    SELECT CASE
        WHEN NEW.status = 'cancelled'
             AND (NEW.cancelled_at IS NULL OR trim(COALESCE(NEW.cancelled_reason, '')) = ''
                  OR trim(COALESCE(NEW.cancelled_by, '')) = '')
        THEN RAISE(ABORT, 'cancelled auction metadata is required')
        WHEN NEW.status <> 'cancelled'
             AND (NEW.cancelled_at IS NOT NULL OR NEW.cancelled_reason IS NOT NULL
                  OR NEW.cancelled_by IS NOT NULL)
        THEN RAISE(ABORT, 'non-cancelled auction cannot have cancellation metadata')
    END;
END;
