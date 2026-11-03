//! 资金拒绝的附加安全日志；不包装错误，不改变事务、重试与失败关闭语义。

use auction_application::FundingError;

pub(crate) const TARGET: &str = "auction::settlement::funding_diagnostics";

/// 只记录固定位置与原错误分类，原错误按值返回；业务标识由安全 instrument span 传递。
pub(crate) fn reject(
    safe_kind: &'static str,
    stage: &'static str,
    error: FundingError,
) -> FundingError {
    tracing::warn!(
        target: TARGET,
        safe_kind,
        cause_kind = error.safe_kind(),
        stage,
        "COD 实收资金化校验拒绝"
    );
    error
}
