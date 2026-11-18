// Public 与 Admin 共用的客户端格式校验。

import { utf8ByteLength } from "../../text";

/** UUID 必须以小写十六进制给出；版本 1..5 与 RFC 4122 variant 固定。 */
const VALID_UUID =
  /^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}(?![\s\S])/;

/** 旧版浏览器 decoder 接受的 UUID：36 字节小写连字符规范形式、版本 1 至 5、RFC 4122 variant。 */
export function validUuid(value: string): boolean {
  return utf8ByteLength(value) === 36 && VALID_UUID.test(value);
}

/** 以转义文本补充单行输入控件会剔除的换行。 */
export function escapedSingleLineInput(value: string): string | null {
  return /[\r\n]/u.test(value) ? JSON.stringify(value) : null;
}
