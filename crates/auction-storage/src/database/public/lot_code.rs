//! 公开 lot 代码的 SQL 片段唯一定义：竞价日志与履约进度的 lot 代码筛选共用同一表达式。

/// 小写的公开 lot 代码：分类前缀加至少两位补零的类别内编号，未知分类回退为 `Lot`。`$equipment`
/// 与 `$lot` 是调用方 FROM 中装备与 lot 行的别名。口径与应用层 `lot_code` 及浏览器的
/// `EQUIPMENT_CATEGORY_CODES` 一致，`postgres_bid_log_lot_code_search_matches_rust_lot_codes`
/// 逐类核对两侧；任一处单独改前缀或补零规则，筛选就会与页面显示的代码对不上。
macro_rules! public_lot_code_sql {
    ($equipment:literal, $lot:literal) => {
        concat!(
            "lower(
                    CASE ",
            $equipment,
            ".category
                        WHEN 'One-Handed' THEN 'One'
                        WHEN 'Two-Handed' THEN 'Two'
                        WHEN 'Staff' THEN 'Sta'
                        WHEN 'Shield' THEN 'Shd'
                        WHEN 'Cloth' THEN 'Clo'
                        WHEN 'Light' THEN 'Lig'
                        WHEN 'Heavy' THEN 'Hea'
                        ELSE 'Lot'
                    END || CASE
                        WHEN ",
            $lot,
            ".category_number < 10 THEN '0' || ",
            $lot,
            ".category_number::text
                        ELSE ",
            $lot,
            ".category_number::text
                    END
                )"
        )
    };
}

pub(crate) use public_lot_code_sql;

#[cfg(test)]
mod tests {
    #[test]
    fn lot_code_binds_every_reference_to_the_caller_aliases() {
        let bid_log = public_lot_code_sql!("e", "l");
        let fulfillment = public_lot_code_sql!("equipment", "lot");
        assert_eq!(
            bid_log.replace("e.category", "equipment.category").replace("l.", "lot."),
            fulfillment
        );
        assert_eq!(fulfillment.matches("lot.category_number").count(), 3);
        assert!(fulfillment.contains("CASE equipment.category"), "{fulfillment}");
    }
}
