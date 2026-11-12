//! 控制台侧栏收起偏好的本地契约。
//! 偏好键固定为 `hv.sidebar`，只接受 `collapsed` / `expanded`；缺失、非法或存储不可用时
//! 回退展开。收起状态纯属展示偏好，不参与路由、权限或任何业务判断。

use crate::storage::{get_item, set_item};

/// 侧栏偏好的 `localStorage` 键；Public 与 Admin 共用一个键，同一 origin 下保持一致。
pub const SIDEBAR_STORAGE_KEY: &str = "hv.sidebar";

const COLLAPSED: &str = "collapsed";
const EXPANDED: &str = "expanded";

/// 严格解析存储值；未知值按展开处理，避免损坏的偏好把导航藏起来。
fn parse(value: Option<&str>) -> bool {
    value == Some(COLLAPSED)
}

/// 读取当前侧栏是否收起；存储不可用或值非法时返回展开。
pub fn load_sidebar_collapsed() -> bool {
    parse(get_item(SIDEBAR_STORAGE_KEY).as_deref())
}

/// 保存侧栏状态；存储失败只影响下次启动恢复，不影响本次切换。
pub fn save_sidebar_collapsed(collapsed: bool) {
    let _ = set_item(SIDEBAR_STORAGE_KEY, if collapsed { COLLAPSED } else { EXPANDED });
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn sidebar_preference_is_a_closed_set() {
        assert!(parse(Some("collapsed")));
        assert!(!parse(Some("expanded")));
        assert!(!parse(None));
        assert!(!parse(Some("")));
        assert!(!parse(Some("true")));
        assert!(!parse(Some("Collapsed")));
    }
}
