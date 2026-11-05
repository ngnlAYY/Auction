-- W12.6 热路径 JSON 键列化：投递 `payload` 的 world/operation/template 三个热键提升为
-- 类型化生成列（typed generated columns）。
-- owner: auction-storage（`deliveries` 的唯一 SQL owner）。
--
-- 背景：领取、发送前门禁、维护扫描、Admin 检索与 SLI 都在 `deliveries.payload ->> …` 上
-- 逐行解压 JSONB。现状基线把这些谓词列为热路径成本；`external_mail_id`、`credits`、`cod`
-- 等其余键只在单条投递的专用路径上按需读取，没有成为整批扫描的过滤条件，因此本迁移只列化
-- 已证明热的三个键，不扩大范围。
--
-- 不变量：
--   1. `payload` 仍是唯一真相。三列都是 `GENERATED ALWAYS AS (payload ->> '…') STORED`：
--      写入方只写 `payload`，列由数据库派生，不能独立写入，因此不存在第二份可漂移的事实。
--      `->>` 对缺失键与 JSON null 都返回 SQL NULL，与迁移前的谓词逐行等价。
--   2. 回填只追加、不改写历史：`ADD COLUMN … STORED` 在停机门禁下整表重写，按既有
--      `payload` 为每一行算出列值；不 UPDATE 任何既有列，`payload`、状态、租约、fencing、
--      审计与冻结快照保持原样。旧程序仍可读写 `payload`，不需要认识新列。
--   3. 两个既有候选索引换成列形式，谓词子集关系保持不变：
--      `deliveries_mailbox_sweep_order` 的世界前导仍是 `COALESCE(world, 'main')`，
--      `deliveries_admin_template_idx` 只覆盖模板非空行（NULL 永远不会匹配等值筛选）。
--      索引只是候选入口，筛选、排序、分页与安全投影仍由查询决定。
--   4. 整行证据比较：人工证明与装备损失把 `to_jsonb(deliveries)` 与冻结快照做等值比较。
--      新增列会出现在整行 JSON 里，因此本版 SQL 在比较两侧同时剔除这三列，使升级前写入的
--      快照继续成立、升级后写入的快照也不因派生列而漂移。
--   5. 权限沿用 `000081` 的表级授权面：新列不需要、也不获得独立 GRANT。
--
-- 等级：breaking。列本身是纯新增，旧程序写入可省略（生成列由数据库维护），但既有整行
--   证据比较在新列存在时会多出三个键，回退到旧程序会因此拒绝原本成立的证明（失败关闭而非
--   误放行）。登记为 breaking，要求停机门禁下同版切换；升级后不得单独回退程序。
--
-- 回填校验（迁移内不猜测）：三列由数据库从 payload 派生，无需也禁止手工修正；迁移后
-- `SELECT count(*) FROM public.deliveries` 与
-- `WHERE payload_world IS DISTINCT FROM payload ->> 'world'` 必须恒为 0，由后续等价性测试
-- 与在线核对复核。

ALTER TABLE public.deliveries
    ADD COLUMN payload_world text GENERATED ALWAYS AS (payload ->> 'world') STORED,
    ADD COLUMN payload_operation text GENERATED ALWAYS AS (payload ->> 'operation') STORED,
    ADD COLUMN payload_template text GENERATED ALWAYS AS (payload ->> 'template') STORED;

-- 维护扫描顺序索引：语义与 000061 相同，只把 JSON 前导换成派生列。
DROP INDEX public.deliveries_mailbox_sweep_order;
CREATE INDEX deliveries_mailbox_sweep_order
    ON public.deliveries ((COALESCE(payload_world, 'main')), created_at, id)
    WHERE status = 'manual_reconcile'
      AND payload_operation IN ('send', 'take')
      AND external_reference IS NULL
      AND sending_at IS NOT NULL
      AND completed_at IS NOT NULL;

-- Admin 模板筛选候选索引：语义与 000087 相同；`payload_template` 为 NULL 的行永远不满足
-- 等值筛选，因此部分谓词从 `payload ? 'template'` 收窄为 `payload_template IS NOT NULL`。
DROP INDEX public.deliveries_admin_template_idx;
CREATE INDEX deliveries_admin_template_idx
    ON public.deliveries (payload_template, created_at DESC, id DESC)
    WHERE payload_template IS NOT NULL;

INSERT INTO public.audit_records
    (id, actor_type, actor_id, action, entity_type, entity_id, result, source, metadata)
VALUES (
    gen_random_uuid(), 'system', 'migration-000094', 'system.migration', 'migration',
    '000094', 'success', 'system',
    jsonb_build_object(
        'summary', 'promote delivery payload world/operation/template to generated typed columns',
        'compatibility', 'breaking')
);

INSERT INTO public.schema_migration_compatibility (version, level)
VALUES (94, 'breaking');
