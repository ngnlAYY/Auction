// 竞价日志 lot 筛选的草稿、地址栏写回与字段校验；履约进度沿用同一校验口径。

import { sleep } from "@auction/web-shared/platform/timer";
import { type Cell, createCell } from "@auction/web-shared/reactive/cell";
import { structuralEquals } from "@auction/web-shared/reactive/equality";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { createTaskScope } from "@auction/web-shared/reactive/tasks";
import { utf8ByteLength } from "@auction/web-shared/text";
import { escapedSingleLineInput } from "@auction/web-shared/ui/validation";
import { createEffect } from "solid-js";

import { usePublicQuerySearch } from "../../pagination/location";
import {
  currentBrowserSearch,
  currentPublicPathname,
  type PublicBoardQuery,
  readPublicQuery,
  updatePublicQueryState,
} from "../../query";
import { rustTrim } from "../../query/text";

const NIL_UUID = "00000000-0000-0000-0000-000000000000";

type LotFilterScope = Pick<PublicBoardQuery, "world" | "auctionId">;

function sameLotFilterScope(left: LotFilterScope, right: LotFilterScope): boolean {
  return left.world === right.world && left.auctionId === right.auctionId;
}

function readLotValues(search: string): string[] {
  return new URLSearchParams(search).getAll("lot");
}

function validLotDraft(value: string): boolean {
  return !/\p{Cc}/u.test(value) && validLotFilter(rustTrim(value));
}

/** 出价记录与履约共用草稿；延时写入只属于输入时已应用的世界、场次与 lot 筛选。 */
export function useLotFilterDraft(
  queryState: Cell<PublicBoardQuery>,
  pathname: string,
  debounceMillis: number,
) {
  const tasks = createTaskScope();
  const search = usePublicQuerySearch();
  const appliedValues = createValueMemo(() => readLotValues(search()));
  const appliedLot = createValueMemo(() => appliedValues()[0] ?? "");
  const duplicate = createValueMemo(() => appliedValues().length > 1);
  const appliedValid = createValueMemo(
    () =>
      !duplicate() &&
      (appliedValues().length === 0 ||
        (rustTrim(appliedLot()) !== "" && validLotDraft(appliedLot()))),
  );
  let appliedScope: LotFilterScope = queryState.peek();
  const draft = createCell(appliedLot());
  const generation = createCell(0);
  const edited = createCell(false);
  const interacted = createCell(false);
  const selection = createValueMemo(() => {
    const query = queryState();
    return { world: query.world, auctionId: query.auctionId, values: appliedValues() };
  });
  const restore = (value: string) => {
    edited.set(false);
    interacted.set(false);
    resetLotDraft(draft, generation, value);
  };
  let previousValues = appliedValues();
  createEffect(selection, (applied) => {
    const scopeChanged = !sameLotFilterScope(applied, appliedScope);
    appliedScope = applied;
    const changed = !structuralEquals(applied.values, previousValues);
    previousValues = applied.values;
    // 换场或换世界必须作废旧输入，即使两边都没有已应用的 lot 筛选。
    // 同范围写回后的值与草稿去空白后相同时，保留用户正在编辑的原文。
    const value = applied.values[0] ?? "";
    if (
      scopeChanged ||
      (changed && rustTrim(draft.peek()) !== value) ||
      applied.values.length > 1
    ) {
      restore(value);
    }
  });
  const draftError = createValueMemo(() => {
    if (!edited()) {
      return duplicate() ? "duplicate" : appliedValid() ? null : "invalid";
    }
    return interacted() && !validLotDraft(draft()) ? "invalid" : null;
  });
  const errorOriginal = createValueMemo(() => {
    const error = draftError();
    if (error === "duplicate") {
      return appliedValues()
        .map((value) => JSON.stringify(value))
        .join(" / ");
    }
    return error === "invalid" ? escapedSingleLineInput(draft()) : null;
  });
  return {
    draft,
    appliedLot,
    appliedValid,
    draftError,
    errorOriginal,
    interacted,
    blur() {
      if (edited.peek()) interacted.set(true);
    },
    input(value: string) {
      const lot = rustTrim(value);
      resetLotDraft(draft, generation, value);
      edited.set(true);
      if (!validLotDraft(value)) {
        return;
      }
      const requested = queryState.peek();
      const rawValues = readLotValues(currentBrowserSearch());
      const requestedGeneration = generation.peek();
      tasks.spawn(async (context) => {
        await sleep(debounceMillis, context.signal);
        if (
          !context.alive() ||
          generation.peek() !== requestedGeneration ||
          currentPublicPathname() !== pathname
        ) {
          return;
        }
        // 地址可能先于查询状态更新；旧草稿不能覆盖另一世界、场次或已应用的 lot。
        const current = readPublicQuery();
        if (
          sameLotFilterScope(requested, current) &&
          structuralEquals(rawValues, readLotValues(currentBrowserSearch())) &&
          !applyLotFilter(
            queryState,
            lot,
            rawValues.length > 0 && (rawValues.length > 1 || rawValues[0] !== lot),
          )
        ) {
          restore(rawValues[0] ?? "");
        }
      });
    },
    clear() {
      // 清除与普通输入共用 URL 提交点；失败时恢复未被改写的已应用值。
      const values = readLotValues(currentBrowserSearch());
      const written =
        currentPublicPathname() === pathname && applyLotFilter(queryState, "", values.length > 0);
      restore(written ? "" : (values[0] ?? ""));
    },
  };
}

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
export function applyLotFilter(
  queryState: Cell<PublicBoardQuery>,
  lot: string,
  force = false,
): boolean {
  const nextLot = lot !== "" ? lot : null;
  if (!force && queryState.peek().bidLogLot === nextLot) {
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
