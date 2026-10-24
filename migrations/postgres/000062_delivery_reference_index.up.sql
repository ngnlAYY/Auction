-- delivery_scheduler 在绑定 Sent MID 前仍须核对升级前未登记 binding 的投递。
-- 非唯一部分索引只缩小历史引用查找范围，不更改旧事实或 Serializable 碰撞保护。
CREATE INDEX deliveries_external_reference_idx
    ON public.deliveries (external_reference)
    WHERE external_reference IS NOT NULL;
