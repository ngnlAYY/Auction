//! 页面内容加载计数的共享上下文与组件级登记。

import type { Cell } from "@auction/web-shared/reactive/cell";
import { createContext, createEffect, onCleanup, useContext } from "solid-js";

/** 外壳持有的全局加载计数；页面只增删自己的编号。 */
export type PublicContentLoading = Cell<ReadonlySet<number>>;

export const PublicContentLoadingContext = createContext<PublicContentLoading>();

let nextContentLoadingId = 0;

function addContentLoading(loading: PublicContentLoading, id: number): void {
  if (loading.peek().has(id)) {
    return;
  }
  loading.update((active) => {
    const next = new Set(active);
    next.add(id);
    return next;
  });
}

function removeContentLoading(loading: PublicContentLoading, id: number): void {
  if (!loading.peek().has(id)) {
    return;
  }
  loading.update((active) => {
    const next = new Set(active);
    next.delete(id);
    return next;
  });
}

/** 组件挂载期间按访问器的值登记加载状态；卸载时无条件移除自己的编号。 */
export function useContentLoading(loading: () => boolean): void {
  const contentLoading = useContext(PublicContentLoadingContext);
  const id = ++nextContentLoadingId;
  createEffect(
    () => loading(),
    (active) => {
      if (active) {
        addContentLoading(contentLoading, id);
      } else {
        removeContentLoading(contentLoading, id);
      }
    },
  );
  onCleanup(() => removeContentLoading(contentLoading, id));
}
