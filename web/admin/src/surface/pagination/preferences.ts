// 每表独立的非敏感展示偏好；失败存储不影响当前页，不读取 Public 偏好。

import { getItem, setItem } from "@auction/web-shared/platform/storage";

/** URL 明确值与本地偏好共用的严格解析；只接受纯十进制正整数，且不超过该表的数量上限。 */
export function parse(value: string, maximum: number | null): number | null {
  if (!asciiDigits(value)) {
    return null;
  }
  const size = Number(value);
  return Number.isSafeInteger(size) && size > 0 && (maximum === null || size <= maximum)
    ? size
    : null;
}

/** 读取该表保存的数量；缺失、损坏或越界时回退 10。 */
export function load(scope: string, maximum: number | null): number {
  const stored = getItem(`hv.admin.page-size:${scope}`);
  return (stored === null ? null : parse(stored, maximum)) ?? 10;
}

/** 只保存当前已应用的数量；存储不可用或写入失败不阻止页面继续使用新数量。 */
export function save(scope: string, size: number): void {
  setItem(`hv.admin.page-size:${scope}`, String(size));
}

function asciiDigits(value: string): boolean {
  if (value.length === 0) {
    return false;
  }
  for (let index = 0; index < value.length; index += 1) {
    const code = value.charCodeAt(index);
    if (code < 48 || code > 57) {
      return false;
    }
  }
  return true;
}
