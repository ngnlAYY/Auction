-- 同一世界中的一个 Users 主体只能对应一个 E-Hentai 论坛账号，避免同名回帖并发建立多重归属。
CREATE UNIQUE INDEX forum_bidder_identities_user_world_unique
    ON public.forum_bidder_identities (world_id, provider, user_id);
