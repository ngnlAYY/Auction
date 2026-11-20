// Admin query 测试的窗口与响应式作用域替身。

import type { QueryState } from "@auction/web-shared/net/query";
import { installFakeWindow } from "@auction/web-shared/router/test-window";
import { createRoot, flush } from "solid-js";

/** 在内存地址栏上挂载 query hook；返回值包含地址替身、状态句柄和拆卸入口。 */
export function mountQueryState<T>(
  initialUrl: string,
  factory: () => QueryState<T>,
): {
  readonly fake: ReturnType<typeof installFakeWindow>["fake"];
  readonly state: QueryState<T>;
  dispose(): void;
} {
  const { fake, restore } = installFakeWindow(initialUrl);
  let disposeRoot = (): void => {};
  const dispose = (): void => {
    try {
      disposeRoot();
    } finally {
      try {
        flush();
      } finally {
        restore();
      }
    }
  };
  try {
    const state = createRoot((disposeCurrentRoot) => {
      disposeRoot = disposeCurrentRoot;
      return factory();
    });
    flush();
    return { fake, state, dispose };
  } catch (error) {
    dispose();
    throw error;
  }
}
