// 控制台侧栏收起偏好的本地契约。
// 偏好键固定为 `hv.sidebar`，只接受 `collapsed` / `expanded`；缺失、非法或存储不可用时
// 回退展开。收起状态纯属展示偏好，不参与路由、权限或任何业务判断。

import { getItem, setItem } from "../storage";

/** 侧栏偏好的 `localStorage` 键；Public 与 Admin 共用一个键，同一 origin 下保持一致。 */
const SIDEBAR_STORAGE_KEY = "hv.sidebar";

const COLLAPSED = "collapsed";
const EXPANDED = "expanded";

/** 严格解析存储值；未知值按展开处理，避免损坏的偏好把导航藏起来。 */
function parse(value: string | null): boolean {
  return value === COLLAPSED;
}

/** 读取当前侧栏是否收起；存储不可用或值非法时返回展开。 */
export function loadSidebarCollapsed(): boolean {
  return parse(getItem(SIDEBAR_STORAGE_KEY));
}

/** 保存侧栏状态；存储失败只影响下次启动恢复，不影响本次切换。 */
export function saveSidebarCollapsed(collapsed: boolean): void {
  setItem(SIDEBAR_STORAGE_KEY, collapsed ? COLLAPSED : EXPANDED);
}
