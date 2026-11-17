//! 公共目录页大小偏好的本地存储。

import { getItem, setItem } from "@auction/web-shared/platform/storage";

import { acceptedPublicPageSize, DEFAULT_PAGE_SIZE } from "./index";

const PAGE_SIZE_KEY = "hv.public.page-size";
const MOVEMENT_PAGE_SIZE_KEY = "hv.public.movement-page-size";

/** URL 与本地存储共用的严格页大小解析；不接受符号、空白、小数或越界值。 */
export function parsePublicPageSize(value: string): number | null {
  if (value.length === 0 || !/^[0-9]+$/u.test(value)) {
    return null;
  }
  const parsed = Number(value);
  return Number.isSafeInteger(parsed) ? acceptedPublicPageSize(parsed) : null;
}

function pageSizeKey(movement: boolean): string {
  return movement ? MOVEMENT_PAGE_SIZE_KEY : PAGE_SIZE_KEY;
}

/** 读取已接受的页大小；缺失、损坏或越界时回退默认值。 */
export function storedPublicPageSize(movement: boolean): number {
  const stored = getItem(pageSizeKey(movement));
  return (stored === null ? null : parsePublicPageSize(stored)) ?? DEFAULT_PAGE_SIZE;
}

/** 只保存合法页大小；存储失败不改变当前页面。 */
export function savePublicPageSize(size: number, movement: boolean): void {
  if (acceptedPublicPageSize(size) === null) {
    return;
  }
  setItem(pageSizeKey(movement), String(size));
}
