// Public 与 Admin 共用的基础组件副作用入口；共享样式由应用入口静态装配。

import { useVisualViewport } from "../../platform/media/viewport";
import { useTableHints } from "./table-hints";

/** 源码中的 BaseStylesheet 与样式资源声明不迁移；这里只安装表格提示和视口副作用。
 * 须在组件初始化阶段调用，内部通过 `onSettled` 注册卸载清理。 */
export function installSharedUi(): void {
  useTableHints();
  useVisualViewport();
}
