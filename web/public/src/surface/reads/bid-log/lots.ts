// 竞价日志 lot 筛选的草稿、地址栏写回与字段校验；履约进度沿用同一校验口径。

import type { Cell } from "@auction/web-shared/reactive/cell";
import { utf8ByteLength } from "@auction/web-shared/text";

import { type PublicBoardQuery, updatePublicQueryState } from "../../query";

const NIL_UUID = "00000000-0000-0000-0000-000000000000";

/** 去空白后的 lot 筛选能否被公开竞价日志接口接受：空（不筛选）、不超过 32 个 ASCII 字母数字
 * 的编号，或规范小写连字符形式的非 nil UUID。口径与服务端一致，其余取值只会得到 400；
 * 履约进度接口沿用同一口径。 */
export function validLotFilter(value: string): boolean {
  if (utf8ByteLength(value) <= 32 && /^[A-Za-z0-9]*$/u.test(value)) {
    return true;
  }
  const canonical = canonicalUuid(value);
  return canonical !== null && canonical !== NIL_UUID && canonical === value;
}

/** 把输入草稿换成新值，并作废在途的防抖写入。 */
export function resetLotDraft(draft: Cell<string>, generation: Cell<number>, value: string): void {
  const next = Math.min(generation.peek() + 1, Number.MAX_SAFE_INTEGER);
  generation.set(next);
  draft.set(value);
}

/** 把合法的 lot 筛选写入地址栏（无历史记录）并回到第一页，写入成功后才提交已应用状态；
 * 失败时保留原状态，之后输入同一值仍会重试。筛选未变时不改动，保留当前页码。 */
export function applyLotFilter(queryState: Cell<PublicBoardQuery>, lot: string): boolean {
  const nextLot = lot !== "" ? lot : null;
  if (queryState.peek().bidLogLot === nextLot) {
    return true;
  }
  return updatePublicQueryState(
    queryState,
    (query) => {
      query.bidLogLot = nextLot;
      query.page = 1;
    },
    "replace",
  );
}

/** 接受 uuid crate 的简单、连字符、花括号与 URN 形式，并输出规范小写连字符 UUID。 */
function canonicalUuid(value: string): string | null {
  let hyphenated = value;
  if (value.length === 38 && value.startsWith("{") && value.endsWith("}")) {
    hyphenated = value.slice(1, -1);
  } else if (value.length === 45 && asciiEqualsIgnoreCase(value.slice(0, 9), "urn:uuid:")) {
    hyphenated = value.slice(9);
  } else if (value.length === 32 && /^[0-9a-fA-F]{32}$/u.test(value)) {
    hyphenated = `${value.slice(0, 8)}-${value.slice(8, 12)}-${value.slice(12, 16)}-${value.slice(16, 20)}-${value.slice(20)}`;
  }
  if (
    !/^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$/u.test(
      hyphenated,
    )
  ) {
    return null;
  }
  return hyphenated.toLowerCase();
}

/** 只折叠 ASCII 字母大小写，避免 Unicode 大小写映射把非 ASCII 前缀误当成 `urn:uuid:`。 */
function asciiEqualsIgnoreCase(value: string, expected: string): boolean {
  if (value.length !== expected.length) {
    return false;
  }
  for (let index = 0; index < expected.length; index += 1) {
    const code = value.charCodeAt(index);
    const folded = code >= 65 && code <= 90 ? code + 32 : code;
    if (folded !== expected.charCodeAt(index)) {
      return false;
    }
  }
  return true;
}
