//! Worker 心跳详情：Admin 据此说明停在待重新核验或待领回的结算退件为何不前进。

use serde_json::Value;

/// 结算退件 Take 可能发生的世界闭集，按名称排序。
const TAKE_WORLDS: [&str; 2] = ["isekai", "main"];

/// 组装一次心跳的详情；每次发布心跳时调用。
///
/// `take_identity_available` 与投递领取、Take 激活用同一判定（当前能否以持久凭据身份执行该世界的
/// 结算退件 Take）。数据库凭据在运行中失效或运行 owner 被撤销后，下一次心跳就不再声明该世界，
/// Admin 详情随之显示“领回能力不可用”，而不是沿用启动时的声明。
pub(super) fn details(
    forum_sync_enabled: bool,
    recheck_worlds: &[&str],
    take_identity_available: impl Fn(&str) -> bool,
) -> Value {
    let take_worlds =
        TAKE_WORLDS.into_iter().filter(|world| take_identity_available(world)).collect::<Vec<_>>();
    serde_json::json!({
        "forum_sync_enabled": forum_sync_enabled,
        "settlement_return_recheck_worlds": recheck_worlds,
        "settlement_return_take_worlds": take_worlds,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::details;

    /// 能力判定每次都重新读取：失去持久凭据身份的世界不再出现在下一次详情里。
    #[test]
    fn take_worlds_follow_the_current_identity_availability() {
        let isekai_available = AtomicBool::new(true);
        let available = |world: &str| match world {
            "isekai" => isekai_available.load(Ordering::SeqCst),
            "main" => true,
            _ => false,
        };

        let before = details(true, &["isekai", "main"], available);
        assert_eq!(
            before,
            serde_json::json!({
                "forum_sync_enabled": true,
                "settlement_return_recheck_worlds": ["isekai", "main"],
                "settlement_return_take_worlds": ["isekai", "main"],
            })
        );

        isekai_available.store(false, Ordering::SeqCst);
        let after = details(true, &["isekai", "main"], available);
        assert_eq!(after["settlement_return_take_worlds"], serde_json::json!(["main"]));
        // 只读核验能力来自配置的读取 gateway，不随 Take 身份变化。
        assert_eq!(
            after["settlement_return_recheck_worlds"],
            serde_json::json!(["isekai", "main"])
        );
    }

    /// 未启用 MoogleMail 或只配置 Cookie 时没有任何世界能冻结身份。
    #[test]
    fn no_identity_declares_no_take_world() {
        let none = details(false, &[], |_| false);
        assert_eq!(none["settlement_return_take_worlds"], serde_json::json!([]));
        assert_eq!(none["settlement_return_recheck_worlds"], serde_json::json!([]));
    }
}
