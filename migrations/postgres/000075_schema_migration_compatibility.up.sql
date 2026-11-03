-- platform/storage owner：登记迁移对旧程序的兼容承诺；已发布 001–074 不回填，缺行即 breaking。
-- 不变量：迁移与本版本登记由 SQLx 同事务提交，历史文件与兼容事实只追加，未知或缺失登记失败关闭。
-- 引入能力的本版保守登记为 breaking；之后的纯新增迁移才可显式声明 compatible。
CREATE TABLE public.schema_migration_compatibility (
    version bigint PRIMARY KEY CHECK (version > 74),
    level text NOT NULL DEFAULT 'breaking' CHECK (level IN ('compatible', 'breaking')),
    recorded_at timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE FUNCTION public.reject_schema_migration_compatibility_mutation()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    RAISE EXCEPTION 'schema migration compatibility is append-only'
        USING ERRCODE = '55000';
END;
$$;

CREATE TRIGGER schema_migration_compatibility_append_only
BEFORE UPDATE OR DELETE ON public.schema_migration_compatibility
FOR EACH ROW EXECUTE FUNCTION public.reject_schema_migration_compatibility_mutation();

CREATE TRIGGER schema_migration_compatibility_no_truncate
BEFORE TRUNCATE ON public.schema_migration_compatibility
FOR EACH STATEMENT EXECUTE FUNCTION public.reject_schema_migration_compatibility_mutation();

INSERT INTO public.schema_migration_compatibility (version, level)
VALUES (75, 'breaking');
