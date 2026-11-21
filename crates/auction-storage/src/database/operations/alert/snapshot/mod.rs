//! Operations alert 水位读取与 signal 观察写入的模块装配。

mod disposition;
mod observe;
pub(super) mod reads;

pub(super) use disposition::apply_read_mode;
pub(super) use observe::{observe_host_postgres, observe_postgres};
pub(super) use reads::snapshot_postgres;

/// 只在 `test-db` 下暴露履约水位与履约终态配对口径的查询原文给基准/EXPLAIN 与口径等价夹具，
/// 保证测量与比较的是同一份 SQL；默认构建与发布制品不包含该出口。
#[cfg(feature = "test-db")]
pub mod benchmark_sql {
    /// 履约水位与长期未领取清单：`$1` 观察时刻，`$2` 超期截止（可为 NULL），`$3` 是否读清单，
    /// `$4` 清单上限。
    pub fn fulfillment_snapshot_sql() -> &'static str {
        super::reads::fulfillment::FULFILLMENT_SNAPSHOT
    }

    /// 履约终态的配对口径（无参数）：同一语句返回迁移前口径与分类口径的失败计数。
    pub fn paired_fulfillment_counts_sql() -> &'static str {
        super::disposition::PAIRED_FULFILLMENT_COUNTS.as_str()
    }
}
