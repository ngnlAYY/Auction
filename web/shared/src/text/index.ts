// 与后端字符串语义对齐的文本原语：排序按 Unicode 码点，长度按 UTF-8 字节。
// JS 的 `<`、`>` 与 `sort()` 按 UTF-16 码元比较，对 U+10000 以上的字符与后端的顺序相反；
// `text.length` 是码元数，不是后端长度上限所用的字节数。

const UTF8_ENCODER = new TextEncoder();

/** 按 Unicode 码点比较两个字符串，结果与后端字符串的 `cmp`（UTF-8 字节序）一致。 */
export function compareText(left: string, right: string): -1 | 0 | 1 {
  let leftIndex = 0;
  let rightIndex = 0;
  while (leftIndex < left.length && rightIndex < right.length) {
    const leftPoint = left.codePointAt(leftIndex) ?? 0;
    const rightPoint = right.codePointAt(rightIndex) ?? 0;
    if (leftPoint !== rightPoint) {
      return leftPoint < rightPoint ? -1 : 1;
    }
    leftIndex += leftPoint > 0xffff ? 2 : 1;
    rightIndex += rightPoint > 0xffff ? 2 : 1;
  }
  if (leftIndex < left.length) {
    return 1;
  }
  return rightIndex < right.length ? -1 : 0;
}

/** 字符串的 UTF-8 字节数，对应后端的 `str::len`。 */
export function utf8ByteLength(text: string): number {
  return UTF8_ENCODER.encode(text).length;
}
