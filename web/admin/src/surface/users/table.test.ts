// 用户搜索表单在同一挂载内保留草稿、恢复筛选并更新实时播报。

import { expect, spyOn, test } from "bun:test";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { flush } from "solid-js";
import type { AdminUserStatusFilter } from "../../api/users";
import {
  field,
  focusControl,
  loadUserFilters,
  onlyElement,
  submitForm,
  withT10View,
} from "../t10-view-fixture.test";

const SEARCH_ERROR = "名称不得超过 256 个 UTF-8 字节，不能含首尾空白或控制字符。";
type Query = readonly [name: string, status: AdminUserStatusFilter, page: number];

async function withSearch(
  run: (ui: {
    readonly query: ReturnType<typeof createCell<Query>>;
    readonly history: ReturnType<typeof createCell<number>>;
    readonly input: ReturnType<typeof field>;
    readonly form: ReturnType<typeof onlyElement>;
    readonly nameError: () => ReturnType<typeof field> | null;
    readonly announcement: ReturnType<typeof field>;
    readonly applied: readonly string[];
  }) => void,
) {
  const Filters = await loadUserFilters();
  const query = createCell<Query>(["玩家甲", "active", 1]);
  const history = createCell(0);
  const applied: string[] = [];
  await withT10View(
    () => {
      const filters = createValueMemo(() => {
        const [name, status] = query();
        return [name, status] as const;
      });
      return Filters({
        applied: filters,
        historyRevision: history,
        onNameApply: (name) => applied.push(name),
        onStatusChange: (status) => query.update(([name]) => [name, status, 1]),
      });
    },
    (document) => {
      run({
        query,
        history,
        input: field(document, "user-name-filter"),
        form: onlyElement(document, "form"),
        nameError: () => document.getElementById("user-name-filter-error"),
        announcement: field(document, "user-name-filter-announcement"),
        applied,
      });
    },
  );
}

test("navigation_restores_applied_filter_but_paging_keeps_unsubmitted_input", async () => {
  const validation = await import("../../api/validation");
  await withSearch(({ query, history, input, form, nameError, applied }) => {
    // Rust 直接同时设置了一段合规草稿和 Name 错误；用一次校验拒绝建立相同状态。
    input.input("未提交的内容");
    flush();
    let restoreValidation: (() => void) | undefined;
    try {
      const safeText = spyOn(validation, "validSafeText");
      restoreValidation = () => safeText.mockRestore();
      safeText.mockReturnValueOnce(false);
      submitForm(form);
      flush();
      expect(safeText).toHaveBeenCalledWith("未提交的内容", 256);
      expect(input.getAttribute("aria-invalid")).toBe("true");
      expect(nameError()?.textContent).toBe(SEARCH_ERROR);
    } finally {
      restoreValidation?.();
    }
    query.set(["玩家甲", "active", 2]);
    flush();
    expect(input.value).toBe("未提交的内容");
    expect(input.getAttribute("aria-invalid")).toBe("true");
    expect(nameError()?.textContent).toBe(SEARCH_ERROR);

    query.set(["玩家甲", "active", 1]);
    history.set(1);
    flush();
    expect(input.value).toBe("玩家甲");
    expect(input.getAttribute("aria-invalid")).toBe("false");
    expect(nameError()).toBeNull();

    for (const [name, status] of [
      ["玩家乙", "blacklisted"],
      ["玩家甲", "active"],
      ["玩家乙", "blacklisted"],
    ] as const) {
      query.set([name, status, 1]);
      flush();
      expect(input.value).toBe(name);
      expect(input.getAttribute("aria-invalid")).toBe("false");
      expect(nameError()).toBeNull();
    }
    expect(applied).toEqual([]);
  });
});

test("invalid_submit_announces_only_when_the_search_already_has_focus", async () => {
  await withSearch(({ input, form, nameError, announcement, applied }) => {
    input.input(" 玩家");
    flush();
    submitForm(form);
    flush();
    expect(input.getAttribute("aria-invalid")).toBe("true");
    expect(nameError()?.textContent).toBe(SEARCH_ERROR);
    expect(announcement.textContent).toBe("");
    expect(applied).toEqual([]);

    focusControl(input);
    submitForm(form);
    flush();
    expect(announcement.textContent).toBe(SEARCH_ERROR);
    expect(nameError()).toBeNull();
    expect(applied).toEqual([]);

    input.input(" 玩家甲");
    flush();
    expect(input.getAttribute("aria-invalid")).toBe("true");
    expect(nameError()?.textContent).toBe(SEARCH_ERROR);
    expect(announcement.textContent).toBe("");
    input.input("玩家甲");
    flush();
    submitForm(form);
    flush();
    expect(applied).toEqual(["玩家甲"]);
    expect(input.getAttribute("aria-invalid")).toBe("false");
    expect(nameError()).toBeNull();
    expect(announcement.textContent).toBe("");
  });
});

test("each_focused_failed_submit_updates_the_live_region", async () => {
  await withSearch(({ input, form, nameError, announcement, applied }) => {
    focusControl(input);
    input.input(" 玩家");
    flush();
    let previous = announcement.querySelector("span");
    expect(previous).toBeNull();
    for (const attempt of [1, 2]) {
      submitForm(form);
      flush();
      const current = announcement.querySelector("span");
      expect(current?.textContent, `第 ${attempt} 次提交应写入播报文字`).toBe(SEARCH_ERROR);
      expect(current).not.toBe(previous);
      expect(applied).toEqual([]);
      previous = current;
    }

    input.input(" 玩家甲");
    flush();
    expect(announcement.textContent).toBe("");
    expect(announcement.querySelector("span")).toBeNull();
    expect(input.getAttribute("aria-invalid")).toBe("true");
    expect(nameError()?.textContent).toBe(SEARCH_ERROR);
  });
});
