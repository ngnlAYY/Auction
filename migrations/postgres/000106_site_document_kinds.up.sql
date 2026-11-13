-- 站点文档：规则文档表由“每种语言一篇规则”扩为“每种文档 × 每种语言一篇”，新增“关于”文档。
-- owner: auction-application（`SiteDocumentKind` 闭集）、auction-storage（public/rules 的读取与发布事务）
--
-- 不变量：
--   1. 文档种类闭集为 rules / about。既有行全部是规则文档，原样归入 rules；版本、标题、正文、
--      发布者与发布时间不改写。
--   2. 文档身份由 locale 改为 (kind, locale)：同一语言的规则与关于互不影响，各自独立做版本 CAS。
--   3. 发布命令账本的每条记录属于一个 (document_kind, locale)；既有记录原样归入 rules。
--      command_id 仍全局唯一，同一 command_id 不能跨文档种类重放。
--   4. 表名沿用 rules_documents / rules_admin_commands（历史名称），不新建表：运行账号的授权、
--      保留策略与备份范围都不变。
--
-- 等级：breaking。旧程序按 locale 单列读写：出现关于文档后按 locale 读取不再唯一，INSERT 不带
-- kind 会违反非空约束，ON CONFLICT (locale) 也不再匹配任何唯一约束。须先停止旧 Public 与
-- Admin 再迁移，回退须按 RECOVERY.md 处理含 breaking 迁移的版本。
LOCK TABLE public.rules_documents, public.rules_admin_commands IN SHARE ROW EXCLUSIVE MODE;

-- 先以默认值回填既有行，再去掉默认值：之后的写入必须显式给出文档种类。
ALTER TABLE public.rules_documents
    ADD COLUMN kind text NOT NULL DEFAULT 'rules'
        CONSTRAINT rules_documents_kind_check CHECK (kind IN ('rules', 'about'));
ALTER TABLE public.rules_documents ALTER COLUMN kind DROP DEFAULT;

ALTER TABLE public.rules_admin_commands
    ADD COLUMN document_kind text NOT NULL DEFAULT 'rules'
        CONSTRAINT rules_admin_commands_document_kind_check
            CHECK (document_kind IN ('rules', 'about'));
ALTER TABLE public.rules_admin_commands ALTER COLUMN document_kind DROP DEFAULT;

ALTER TABLE public.rules_admin_commands
    DROP CONSTRAINT rules_admin_commands_locale_fkey;

ALTER TABLE public.rules_documents
    DROP CONSTRAINT rules_documents_pkey,
    ADD CONSTRAINT rules_documents_pkey PRIMARY KEY (kind, locale);

ALTER TABLE public.rules_admin_commands
    ADD CONSTRAINT rules_admin_commands_document_fkey
        FOREIGN KEY (document_kind, locale)
        REFERENCES public.rules_documents(kind, locale) ON DELETE RESTRICT;

INSERT INTO public.audit_records
    (id, actor_type, actor_id, action, entity_type, entity_id, result, source, metadata)
VALUES (
    gen_random_uuid(), 'system', 'migration-000106', 'system.migration', 'migration',
    '000106', 'success', 'system',
    jsonb_build_object(
        'summary', 'rules documents become site documents keyed by kind and locale',
        'compatibility', 'breaking')
);

INSERT INTO public.schema_migration_compatibility (version, level)
VALUES (106, 'breaking');
