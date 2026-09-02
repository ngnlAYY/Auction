-- SQLite Lite 不提供论坛导入，但 Admin 仍使用同一用户安全投影。
UPDATE users
SET username = game_player_id,
    state_version = state_version + 1,
    updated_at = CURRENT_TIMESTAMP
WHERE game_player_id IS NOT NULL
  AND username <> game_player_id;

ALTER TABLE users ADD COLUMN forum_account_id TEXT
    CHECK (
        forum_account_id IS NULL
        OR (
            length(forum_account_id) BETWEEN 1 AND 32
            AND forum_account_id NOT GLOB '*[^0-9]*'
            AND substr(forum_account_id, 1, 1) BETWEEN '1' AND '9'
        )
    );

CREATE UNIQUE INDEX users_forum_account_id_unique
    ON users (forum_account_id)
    WHERE forum_account_id IS NOT NULL;

CREATE TRIGGER users_name_compatibility_insert
BEFORE INSERT ON users
WHEN NEW.game_player_id IS NOT NULL AND NEW.game_player_id <> NEW.username
BEGIN
    SELECT RAISE(ABORT, 'game_player_id must mirror username');
END;

CREATE TRIGGER users_name_compatibility_update
BEFORE UPDATE OF username, game_player_id ON users
WHEN NEW.game_player_id IS NOT NULL AND NEW.game_player_id <> NEW.username
BEGIN
    SELECT RAISE(ABORT, 'game_player_id must mirror username');
END;
