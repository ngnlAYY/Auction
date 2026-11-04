-- storage owner：CLI 运维审计明确区分 Admin 来源，并保留主机操作者 metadata。
-- 只扩展追加式审计的新记录来源，不修改历史行、动作、实体、结果或操作者说明。
-- breaking：旧程序与旧 Admin 前端拒绝 cli 来源；升级必须遵循停机迁移门禁。
ALTER TABLE public.audit_records DROP CONSTRAINT audit_records_source_check;
ALTER TABLE public.audit_records ADD CONSTRAINT audit_records_source_check
    CHECK (source IN ('public', 'admin', 'worker', 'system', 'cli'));

INSERT INTO public.schema_migration_compatibility (version, level)
VALUES (78, 'breaking');
