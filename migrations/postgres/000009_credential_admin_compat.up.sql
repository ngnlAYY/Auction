-- A32 凭据管理的追加兼容修正。
--
-- 000004 已发布，不能直接修改。旧版 Admin 支持 forum_cookie；删除凭据后仍需保留
-- credential_admin_commands 命令账本，因此删除引用必须变为 SET NULL。

ALTER TABLE public.credential_keys
    DROP CONSTRAINT IF EXISTS credential_keys_credential_kind_check;

ALTER TABLE public.credential_keys
    ADD CONSTRAINT credential_keys_credential_kind_check
    CHECK (credential_kind IN ('mail_cookie', 'forum_cookie', 'external_api'));

ALTER TABLE public.credential_admin_commands
    ALTER COLUMN credential_id DROP NOT NULL;

ALTER TABLE public.credential_admin_commands
    DROP CONSTRAINT IF EXISTS credential_admin_commands_credential_id_fkey;

ALTER TABLE public.credential_admin_commands
    ADD CONSTRAINT credential_admin_commands_credential_id_fkey
    FOREIGN KEY (credential_id) REFERENCES public.credential_keys(id) ON DELETE SET NULL;
