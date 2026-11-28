-- 000024 把 `failed` 且从未写入帖子内容的论坛主题误归为 legacy_global。
-- owner: auction-storage
--
-- 不变量：
--   1. 只有 `pending` 且 thread/title/body 全空的行归入 category_v1；`failed` 但从未写入任何
--      帖子内容、没有 thread ID 与 URL 的行同样没有可解释的全局编号。
--   2. 本次只重分类能够由“无内容”证明的行；有 thread/title/body 的 failed 行保持 legacy_global。
--   3. 不改写任何帖子内容、状态或失败信息。
--
-- 等级：breaking。重分类会改变这些历史投影后续使用的编号方案，须与同版程序一起切换。

LOCK TABLE public.auction_forum_sync IN SHARE ROW EXCLUSIVE MODE;

UPDATE public.auction_forum_sync
SET numbering_scheme = 'category_v1'
WHERE numbering_scheme = 'legacy_global'
  AND status = 'failed'
  AND thread_id IS NULL
  AND thread_url IS NULL
  AND title IS NULL
  AND body IS NULL;

INSERT INTO public.schema_migration_compatibility (version, level)
VALUES (150, 'breaking');
