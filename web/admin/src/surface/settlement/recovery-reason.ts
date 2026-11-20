// 恢复确认理由的端口校验与本地化提示。

import { err, ok, type Result, unreachable } from "@auction/web-shared/result";
import { utf8ByteLength } from "@auction/web-shared/text";
import { t } from "@auction/web-shared/ui/i18n";

// 恢复确认应用端口检查 trim 后将要提交的理由，内部换行是合法输入。
const MAX_RECOVERY_REASON_BYTES = 512;

/** 恢复理由校验错误：裁剪后为空，或裁剪后的 UTF-8 字节数超过上限。 */
export type RecoveryReasonIssue = "required" | "too_long";

/** 把恢复理由的闭集错误映射为本地化提示。 */
export function recoveryReasonIssueMessage(issue: RecoveryReasonIssue): string {
  switch (issue) {
    case "required":
      return t("admin-field-required");
    case "too_long":
      return t("admin-field-max-utf8-bytes", { max: MAX_RECOVERY_REASON_BYTES });
    default:
      return unreachable(issue);
  }
}

/**
 * 按恢复动作校验并规范化理由：`reclaimed` 固定提交空字符串；其余动作按 Rust 的
 * `str::trim` 裁剪，空值报 `required`，裁剪后按 UTF-8 字节数判断是否超过 512 字节。
 */
export function validateRecoveryReason(
  action: string,
  reason: string,
): Result<string, RecoveryReasonIssue> {
  if (action === "reclaimed") {
    return ok("");
  }
  const trimmed = rustTrim(reason);
  if (trimmed === "") {
    return err("required");
  }
  if (utf8ByteLength(trimmed) > MAX_RECOVERY_REASON_BYTES) {
    return err("too_long");
  }
  return ok(trimmed);
}

/** Rust `str::trim` 的空白集合；JS `trim` 会额外去掉 U+FEFF。 */
function rustTrim(value: string): string {
  const characters = Array.from(value);
  let start = 0;
  while (start < characters.length && isRustWhitespace(characters[start] ?? "")) {
    start += 1;
  }
  let end = characters.length;
  while (end > start && isRustWhitespace(characters[end - 1] ?? "")) {
    end -= 1;
  }
  return characters.slice(start, end).join("");
}

function isRustWhitespace(character: string): boolean {
  const code = character.codePointAt(0) ?? 0;
  return (
    (code >= 0x09 && code <= 0x0d) ||
    code === 0x20 ||
    code === 0x85 ||
    code === 0xa0 ||
    code === 0x1680 ||
    (code >= 0x2000 && code <= 0x200a) ||
    code === 0x2028 ||
    code === 0x2029 ||
    code === 0x202f ||
    code === 0x205f ||
    code === 0x3000
  );
}
