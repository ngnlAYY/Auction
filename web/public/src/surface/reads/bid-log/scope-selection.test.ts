// 世界范围选择身份：范围与序号一起变化，范围不变时下游不会收到新值。

import { expect, test } from "bun:test";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createEffect, createRoot, flush } from "solid-js";

import type { WorldScope } from "../../query";
import {
  createScopeSelection,
  nextScopeSelection,
  type ScopeSelection,
  sameScopeSelection,
} from "./scope-selection";

const INITIAL: ScopeSelection = { scope: "all", generation: 0 };

test("scope_generation_stays_when_scope_is_unchanged：范围不变时序号与对象都不变", () => {
  expect(nextScopeSelection(INITIAL, "all")).toBe(INITIAL);
  expect(nextScopeSelection(INITIAL, "all").generation).toBe(0);
});

test("scope_generation_advances_when_scope_changes：范围变化时序号加一", () => {
  const main = nextScopeSelection(INITIAL, "main");
  expect(main).toEqual({ scope: "main", generation: 1 });
});

test("scope_generation_differs_after_leaving_and_returning：切走再切回仍得到新序号", () => {
  const isekai = nextScopeSelection(INITIAL, "isekai");
  const returned = nextScopeSelection(isekai, "all");
  expect(returned).toEqual({ scope: "all", generation: 2 });
  expect(sameScopeSelection(returned, INITIAL)).toBe(false);
});

test("createScopeSelection_emits_one_identity_per_scope_change：派生值随世界范围一次更新", () => {
  createRoot((dispose) => {
    const scope = createCell<WorldScope>("all");
    const selection = createScopeSelection(scope);
    const seen: ScopeSelection[] = [];
    createEffect(
      () => selection(),
      (current) => {
        seen.push(current);
      },
    );
    flush();
    // createCell 的等值写入仍通知；选择身份按范围去重，不产生新值。
    scope.set("all");
    flush();
    scope.set("isekai");
    flush();
    scope.set("isekai");
    flush();
    scope.set("all");
    flush();
    expect(seen).toEqual([
      { scope: "all", generation: 0 },
      { scope: "isekai", generation: 1 },
      { scope: "all", generation: 2 },
    ]);
    dispose();
  });
});
