// 公开寄售页在可见和恢复焦点时的有界重读，离开页面时撤销定时器。

import { createTimeoutFuture, type TimeoutFuture } from "@auction/web-shared/platform/timer";
import { documentIsVisible } from "@auction/web-shared/platform/visibility";
import { createEffect } from "solid-js";

const REFRESH_MILLIS = 30_000;

/** 只有已开启的寄售页面定期重读；窗口焦点和文档可见性恢复也可复核能力。 */
export function useConsignmentRefresh(enabled: () => boolean, refresh: () => void): void {
  createEffect(enabled, (poll) => {
    if (typeof window === "undefined" || typeof document === "undefined") return;

    let timer: TimeoutFuture | null = null;
    const runRefresh = () => {
      try {
        refresh();
      } catch {
        // 单次刷新失败不能终止后续轮询；页面自身的错误态由回调负责。
      }
    };
    const schedule = () => {
      timer?.cancel();
      timer = null;
      if (!poll || !documentIsVisible()) return;
      const current = createTimeoutFuture(REFRESH_MILLIS);
      timer = current;
      void current.promise.then(() => {
        if (timer !== current) return;
        if (documentIsVisible()) runRefresh();
        schedule();
      });
    };
    const restored = () => {
      if (documentIsVisible()) runRefresh();
      schedule();
    };
    const visibilityChanged = () => {
      if (documentIsVisible()) restored();
      else schedule();
    };

    schedule();
    window.addEventListener("focus", restored);
    document.addEventListener("visibilitychange", visibilityChanged);
    return () => {
      window.removeEventListener("focus", restored);
      document.removeEventListener("visibilitychange", visibilityChanged);
      timer?.cancel();
      timer = null;
    };
  });
}
