-- auction-storage owner：锁定已编号 lot 的装备分类身份。
-- owner: auction-storage
-- 不变量：lot.category_key 与其 equipment.category 一致；已有 lot 引用装备后，分类不可漂移。
-- Breaking：新增关系约束会拒绝已编号装备的分类变更；升级前停止写入角色。
LOCK TABLE public.equipment, public.lots IN SHARE ROW EXCLUSIVE MODE;

DO $block$
BEGIN
    IF EXISTS (
        SELECT 1
        FROM public.lots AS lot
        JOIN public.equipment AS equipment ON equipment.id = lot.equipment_id
        WHERE lot.category_key IS DISTINCT FROM equipment.category
    ) THEN
        RAISE EXCEPTION '已有 lot 的分类快照与 equipment 分类不一致；必须先核对'
            USING ERRCODE = '23514';
    END IF;
END
$block$;

ALTER TABLE public.equipment
    ADD CONSTRAINT equipment_id_category_000128 UNIQUE (id, category);

ALTER TABLE public.lots
    ADD CONSTRAINT lots_equipment_category_000128
        FOREIGN KEY (equipment_id, category_key)
        REFERENCES public.equipment (id, category)
        ON UPDATE RESTRICT ON DELETE RESTRICT;

INSERT INTO public.audit_records
    (id, actor_type, actor_id, action, entity_type, entity_id, result, source, metadata)
VALUES (
    gen_random_uuid(), 'system', 'migration-000128', 'system.migration', 'migration',
    '000128', 'success', 'system',
    jsonb_build_object(
        'summary', 'lot equipment category identity constraint',
        'compatibility', 'breaking')
);

INSERT INTO public.schema_migration_compatibility (version, level)
VALUES (128, 'breaking');
