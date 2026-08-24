-- Admin Users owner 的密钥信封与命令账本；两张表均只保存密文、摘要和安全投影。
CREATE TABLE user_login_key_envelopes (
    user_id BLOB PRIMARY KEY REFERENCES users(id) ON DELETE RESTRICT,
    ciphertext BLOB NOT NULL CHECK (length(ciphertext) > 0),
    nonce BLOB NOT NULL CHECK (length(nonce) = 12),
    encryption_key_id TEXT NOT NULL CHECK (trim(encryption_key_id) = encryption_key_id
        AND encryption_key_id <> ''),
    fingerprint TEXT NOT NULL CHECK (trim(fingerprint) = fingerprint AND fingerprint <> ''),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    CHECK (updated_at >= created_at)
);

CREATE TABLE user_admin_commands (
    command_id BLOB PRIMARY KEY,
    command_kind TEXT NOT NULL CHECK (command_kind IN (
        'create', 'rename', 'rename_revert', 'set_game_player_id',
        'revoke_key', 'reissue_key', 'blacklist', 'unblacklist'
    )),
    request_digest BLOB NOT NULL CHECK (length(request_digest) = 32),
    user_id BLOB NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    result_payload TEXT NOT NULL CHECK (json_valid(result_payload)
        AND json_type(result_payload) = 'object'),
    response_ciphertext BLOB,
    response_nonce BLOB,
    response_encryption_key_id TEXT,
    response_fingerprint TEXT,
    created_at TEXT NOT NULL,
    CHECK ((response_ciphertext IS NULL AND response_nonce IS NULL
            AND response_encryption_key_id IS NULL AND response_fingerprint IS NULL)
        OR (response_ciphertext IS NOT NULL AND length(response_ciphertext) > 0
            AND response_nonce IS NOT NULL AND length(response_nonce) = 12
            AND response_encryption_key_id IS NOT NULL
            AND trim(response_encryption_key_id) = response_encryption_key_id
            AND response_encryption_key_id <> ''
            AND response_fingerprint IS NOT NULL
            AND trim(response_fingerprint) = response_fingerprint
            AND response_fingerprint <> ''))
);

CREATE INDEX user_admin_commands_user_idx
    ON user_admin_commands (user_id, created_at, command_id);
