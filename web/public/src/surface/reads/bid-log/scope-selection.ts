// 世界范围与本次选择的序号：范围与代际必须作为同一个值一起变化，目录资源只依赖这个值。

import { createValueMemo } from "@auction/web-shared/reactive/memo";
import type { Accessor } from "solid-js";
import { untrack } from "solid-js";

import type { WorldScope } from "../../query";

/** 一次世界范围选择的身份；`generation` 只在范围变化时推进。 */
export interface ScopeSelection {
  readonly scope: WorldScope | null;
  readonly generation: number;
}

/** 与上一次选择比较：范围不变时原样返回同一个值，范围变化时序号加一。纯函数，调用方持有
 * 上一次的值；派生函数里不写其他响应式状态。 */
export function nextScopeSelection(
  previous: ScopeSelection,
  scope: WorldScope | null,
): ScopeSelection {
  if (scope === previous.scope) {
    return previous;
  }
  return {
    scope,
    generation: Math.min(previous.generation + 1, Number.MAX_SAFE_INTEGER),
  };
}

/** 两个身份是否属于同一次范围选择：范围相同但序号不同（切走再切回）不算同一次。 */
export function sameScopeSelection(left: ScopeSelection, right: ScopeSelection): boolean {
  return left.scope === right.scope && left.generation === right.generation;
}

/** 展示目录只要求世界选择身份一致；发起重读另校验当前刷新代号。 */
export function directoryForScope<T extends { readonly selection: ScopeSelection }>(
  snapshot: T | null | undefined,
  selection: ScopeSelection,
): T | null {
  return snapshot != null && sameScopeSelection(snapshot.selection, selection) ? snapshot : null;
}

/** 由世界范围派生选择身份；上一次选择只保存在闭包中。范围不变时返回同一个值，下游不会
 * 重跑；范围变化时一次更新同时带出新范围与新序号。 */
export function createScopeSelection(scope: Accessor<WorldScope | null>): Accessor<ScopeSelection> {
  let previous: ScopeSelection = { scope: untrack(scope), generation: 0 };
  return createValueMemo(() => {
    previous = nextScopeSelection(previous, scope());
    return previous;
  });
}
