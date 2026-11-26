//! 名称片段到 `ILIKE` 字面包含模式的转换，价格历史与寄售目录共用。

/// 把每个片段转义 `\`、`%`、`_` 后包成 `%片段%`，供 `ILIKE ALL(...)` 按字面包含匹配；
/// PostgreSQL 的 `LIKE` 默认以反斜杠为转义符，先转义反斜杠才不会吞掉后两者的转义。
pub(crate) fn contains_patterns(terms: &[String]) -> Vec<String> {
    terms
        .iter()
        .map(|term| {
            format!("%{}%", term.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"))
        })
        .collect()
}
