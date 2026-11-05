-- owner: auction-storage（`deliveries` 的唯一 SQL owner）。
--
-- W4.5 Admin 投递检索：列表新增收件人、参考编号、模板、发送类别、世界与创建时间筛选。
-- 世界已有 `deliveries_mailbox_sweep_order`（`COALESCE(payload->>'world','main')` 前导），
-- 其余维度此前都是顺序扫描。本迁移只补四个只读候选索引：
--   1. 收件人精确匹配（ASCII 大小写不敏感）后按创建时间倒序分页；
--   2. 冻结模板键匹配后按创建时间倒序分页（部分索引，跳过无 `template` 键的 Return/Take）；
--   3. 结算义务参考编号（`origin_id`）匹配；只覆盖两种结算来源；
--   4. 发送类别（`public.delivery_send_category` 是不可变函数）匹配后按创建时间倒序分页。
--
-- 不变量：索引只是候选入口。筛选语义、计数、排序、分页上限与安全投影仍由
-- `delivery_admin/reads.rs` 的 WHERE/ORDER BY 与 mapping 校验决定；金额、状态迁移、租约、
-- 审计、事件、冻结载荷与发送节奏均不变，也不新增任何写入。
--
-- compatible：纯新增普通索引，旧程序不读取、不依赖，读写结果不受影响。
CREATE INDEX deliveries_admin_recipient_idx
    ON public.deliveries (lower(recipient), created_at DESC, id DESC);

CREATE INDEX deliveries_admin_template_idx
    ON public.deliveries ((payload ->> 'template'), created_at DESC, id DESC)
    WHERE payload ? 'template';

CREATE INDEX deliveries_admin_origin_id_idx
    ON public.deliveries (origin_id, created_at DESC, id DESC)
    WHERE origin_kind IN ('settlement', 'settlement_recovery_action');

CREATE INDEX deliveries_admin_category_idx
    ON public.deliveries (
        public.delivery_send_category(origin_kind, payload),
        created_at DESC,
        id DESC
    );

INSERT INTO public.schema_migration_compatibility (version, level)
VALUES (87, 'compatible');
