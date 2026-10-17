-- 凭据 owner：只保存邮件账号的预期完整名称；不持久化 verified 状态或远端证明。
ALTER TABLE public.credential_keys
    ADD COLUMN player_name text,
    ADD CONSTRAINT credential_keys_player_name_check CHECK (
        player_name IS NULL OR (
            credential_kind = 'mail_cookie'
            AND octet_length(player_name) BETWEEN 1 AND 256
            AND player_name = btrim(player_name)
            AND player_name !~ '[[:cntrl:]]'
        )
    );

COMMENT ON COLUMN public.credential_keys.player_name IS
    '邮件 Cookie 的预期完整玩家名称，非身份验证结果；旧记录保持 NULL';
