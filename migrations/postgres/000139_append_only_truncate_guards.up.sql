-- 追加式事实表在 TRUNCATE 下同样不可删除；行级触发器不拦截 TRUNCATE，须补语句级触发器。
-- owner: auction-storage
--
-- 不变量：
--   1. `public.bids` 是被接受的竞价事实，`public.audit_records` 是审计账本；二者已有拒绝
--      UPDATE/DELETE 的行级触发器，但 PostgreSQL 的 TRUNCATE 不触发行级触发器。
--   2. 迁移只新增拒绝 TRUNCATE 的语句级触发器，复用既有 `reject_bid_fact_mutation()` 与
--      `reject_audit_mutation()`，不改写任何行、索引或既往约束。
--   3. 运行账号本身没有 TRUNCATE 权限（000081/000122）；本迁移收口的是对象 owner 或维护
--      会话误用 `TRUNCATE ... CASCADE` 抹掉历史事实的路径。
--
-- 等级：breaking。新增会拒绝既有 DELETE 之外的整表清空语义，须与同版程序在停机门禁下切换。

CREATE TRIGGER bids_no_truncate
BEFORE TRUNCATE ON public.bids
FOR EACH STATEMENT EXECUTE FUNCTION public.reject_bid_fact_mutation();

CREATE TRIGGER audit_records_no_truncate
BEFORE TRUNCATE ON public.audit_records
FOR EACH STATEMENT EXECUTE FUNCTION public.reject_audit_mutation();

INSERT INTO public.schema_migration_compatibility (version, level)
VALUES (139, 'breaking');
