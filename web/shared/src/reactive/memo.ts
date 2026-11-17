// 按值去重的派生值：结果与上一次按值相等时不通知下游，避免等价的查询条件触发重复请求。

import type { Accessor } from "solid-js";
import { createMemo } from "solid-js";
import { structuralEquals } from "./equality";

/** 创建按结构相等去重的派生值；compute 内读取的响应式值即依赖。 */
export function createValueMemo<T>(compute: () => T): Accessor<T> {
  return createMemo(compute, { equals: structuralEquals });
}
