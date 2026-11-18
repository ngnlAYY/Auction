// 公开装备分类的多选值规范化、拆分与切换。Rust 版仍是单选；这里只服务 Solid 版看板。

import { EQUIPMENT_CATEGORY_CODES } from "@auction/contracts/dto/public/catalog/lot";

const CATEGORY_KEYS: readonly string[] = EQUIPMENT_CATEGORY_CODES.map(([key]) => key);

/** 把地址栏的 type 取值解析成规范化的逗号列表；任何一项不在闭集内、出现空项或重复时返回 null。 */
export function normalizeCatalogCategories(value: string): string | null {
  const keys = value.split(",");
  const selected = new Set(keys);
  if (selected.size !== keys.length || keys.some((key) => !CATEGORY_KEYS.includes(key))) {
    return null;
  }
  // 参数顺序不影响查询结果；地址栏与响应解码共用分类表的固定顺序。
  return CATEGORY_KEYS.filter((key) => selected.has(key)).join(",");
}

/** "all" 或 null → 空数组；否则拆成分类键数组（已规范化）。 */
export function catalogCategoryKeys(active: string | null): readonly string[] {
  return active === null || active === "all" ? [] : active.split(",");
}

/** 切换一个分类的选中状态，返回新的当前值："all" 或规范化的逗号列表。 */
export function toggleCatalogCategory(active: string, category: string): string {
  const selected = new Set(catalogCategoryKeys(active));
  if (selected.has(category)) {
    selected.delete(category);
  } else {
    selected.add(category);
  }
  return catalogCategoryValue(selected);
}

/** 面板关闭时的提交值：草稿与已应用值相同返回 null，否则返回规范化后的当前值。 */
export function catalogCategoryCommitValue(active: string, draft: string): string | null {
  const appliedValue = catalogCategoryValue(new Set(catalogCategoryKeys(active)));
  const draftValue = catalogCategoryValue(new Set(catalogCategoryKeys(draft)));
  return appliedValue === draftValue ? null : draftValue;
}

/** 手机分类面板一次关闭后的界面状态。
 * 写入失败时目录表不会随查询重读卸载，因此必须撤销“重新挂载后归还焦点”的请求并重新展开
 * 面板，保留草稿供用户再次提交；成功写入才登记请求，未改动则立即聚焦开关。 */
export function catalogCategoryCloseOutcome(
  submission: string | null,
  writeSucceeded: boolean,
): {
  readonly panelOpen: boolean;
  readonly focusRequest: boolean;
  readonly focusToggleNow: boolean;
} {
  if (submission === null) {
    return { panelOpen: false, focusRequest: false, focusToggleNow: true };
  }
  if (!writeSucceeded) {
    return { panelOpen: true, focusRequest: false, focusToggleNow: false };
  }
  return { panelOpen: false, focusRequest: true, focusToggleNow: false };
}

/** Escape 只有分类开关仍有实际渲染盒时才提交草稿；布局在桌面断点隐藏手机控件后，
 * 文档级监听仍可能收到按键，此时只能放弃隐藏草稿，不能写入查询或抢占焦点。 */
export function catalogCategoryEscapeAction(toggleRectCount: number): "submit" | "cancel" {
  return toggleRectCount > 0 ? "submit" : "cancel";
}

/** 目录重读后是否应把焦点还给分类开关：只有焦点仍停在文档起点（body）时才归还；
 * 用户已经移到其他可聚焦元素时放弃，避免抢走键盘焦点。 */
export function shouldRestoreCatalogCategoryFocus(
  activeElement: object | null,
  body: object | null,
): boolean {
  return activeElement === body;
}

function catalogCategoryValue(selected: ReadonlySet<string>): string {
  const keys = CATEGORY_KEYS.filter((key) => selected.has(key));
  return keys.length === 0 ? "all" : keys.join(",");
}
