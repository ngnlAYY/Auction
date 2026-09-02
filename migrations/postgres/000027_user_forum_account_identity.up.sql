-- MM/页面名称只保留 username 为权威值；旧 game_player_id 暂作数据库兼容镜像。
DO $$
BEGIN
    IF EXISTS (
        SELECT 1
        FROM public.users AS target
        JOIN public.users AS occupied
          ON lower(occupied.username) = lower(target.game_player_id)
         AND occupied.id <> target.id
        WHERE target.game_player_id IS NOT NULL
          AND target.username IS DISTINCT FROM target.game_player_id
    ) THEN
        RAISE EXCEPTION 'cannot consolidate username: a game_player_id is owned by another user';
    END IF;
END
$$;

UPDATE public.users
SET username = game_player_id,
    state_version = state_version + 1,
    updated_at = transaction_timestamp()
WHERE game_player_id IS NOT NULL
  AND username IS DISTINCT FROM game_player_id;

ALTER TABLE public.users
    ADD COLUMN forum_account_id text,
    ADD CONSTRAINT users_name_compatibility_check
        CHECK (game_player_id IS NULL OR game_player_id = username),
    ADD CONSTRAINT users_forum_account_id_check
        CHECK (
            forum_account_id IS NULL
            OR (
                octet_length(forum_account_id) BETWEEN 1 AND 32
                AND forum_account_id ~ '^[1-9][0-9]*$'
            )
        );

DO $$
BEGIN
    IF EXISTS (
        SELECT identity.external_account_id
        FROM public.forum_bidder_identities AS identity
        WHERE identity.provider = 'ehentai'
        GROUP BY identity.external_account_id
        HAVING count(DISTINCT identity.user_id) <> 1
    ) THEN
        RAISE EXCEPTION 'one E-Hentai forum account ID is linked to multiple users';
    END IF;
    IF EXISTS (
        SELECT identity.user_id
        FROM public.forum_bidder_identities AS identity
        WHERE identity.provider = 'ehentai'
        GROUP BY identity.user_id
        HAVING count(DISTINCT identity.external_account_id) <> 1
    ) THEN
        RAISE EXCEPTION 'one user is linked to multiple E-Hentai forum account IDs';
    END IF;
END
$$;

UPDATE public.users AS account
SET forum_account_id = identity.external_account_id,
    state_version = account.state_version + 1,
    updated_at = transaction_timestamp()
FROM (
    SELECT user_id, min(external_account_id) AS external_account_id
    FROM public.forum_bidder_identities
    WHERE provider = 'ehentai'
    GROUP BY user_id
) AS identity
WHERE account.id = identity.user_id
  AND account.forum_account_id IS DISTINCT FROM identity.external_account_id;

CREATE UNIQUE INDEX users_forum_account_id_unique
    ON public.users (forum_account_id)
    WHERE forum_account_id IS NOT NULL;

-- 两张 owner 表必须在事务提交时仍指向同一个 showuser；约束延迟执行，允许 Admin 在同一事务
-- 原子修正 Users 与跨世界映射，而不暴露中间分叉状态。
CREATE FUNCTION public.enforce_forum_identity_user_account_id()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
    current_user_id uuid;
    current_external_account_id text;
    account_forum_id text;
BEGIN
    -- 延迟触发器可能为同一行排队多次；只核对提交前的当前行，不能使用中间版本 NEW。
    SELECT user_id, external_account_id
    INTO current_user_id, current_external_account_id
    FROM public.forum_bidder_identities
    WHERE id = NEW.id;
    IF NOT FOUND THEN
        RETURN NULL;
    END IF;

    SELECT forum_account_id
    INTO account_forum_id
    FROM public.users
    WHERE id = current_user_id;

    IF account_forum_id IS DISTINCT FROM current_external_account_id THEN
        RAISE EXCEPTION 'forum identity and user forum account ID must match';
    END IF;
    RETURN NULL;
END
$$;

CREATE FUNCTION public.enforce_user_forum_identity_account_id()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
    current_forum_account_id text;
BEGIN
    -- 与映射侧相同，只检查当前 Users 行，允许同一事务内多次修正后以最终值提交。
    SELECT forum_account_id
    INTO current_forum_account_id
    FROM public.users
    WHERE id = NEW.id;
    IF NOT FOUND THEN
        RETURN NULL;
    END IF;

    IF EXISTS (
        SELECT 1
        FROM public.forum_bidder_identities AS identity
        WHERE identity.user_id = NEW.id
          AND identity.provider = 'ehentai'
          AND identity.external_account_id IS DISTINCT FROM current_forum_account_id
    ) THEN
        RAISE EXCEPTION 'user forum account ID and forum identities must match';
    END IF;
    RETURN NULL;
END
$$;

CREATE CONSTRAINT TRIGGER forum_identity_user_account_id_check
AFTER INSERT OR UPDATE ON public.forum_bidder_identities
DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW
EXECUTE FUNCTION public.enforce_forum_identity_user_account_id();

CREATE CONSTRAINT TRIGGER user_forum_identity_account_id_check
AFTER INSERT OR UPDATE ON public.users
DEFERRABLE INITIALLY DEFERRED
FOR EACH ROW
EXECUTE FUNCTION public.enforce_user_forum_identity_account_id();

COMMENT ON COLUMN public.users.forum_account_id IS
    'E-Hentai showuser 的稳定十进制账号 ID；不得作为 MoogleMail 收件人名称。';
COMMENT ON CONSTRAINT users_name_compatibility_check ON public.users IS
    'game_player_id 仅是旧契约兼容镜像；username 是唯一可变玩家/MM 名称。';
