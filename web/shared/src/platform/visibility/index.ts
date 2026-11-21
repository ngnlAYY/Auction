// 页面可见性共享原语：隐藏期间轮询与重连退避挂起而非重置，恢复可见后继续。
// Public 外壳与 Admin 自动轮询共用；是否在恢复可见时补偿刷新由调用方回调决定。

import { sleep } from "../timer";

/** 睡眠分片粒度：足够细以便在隐藏时尽快转入事件驱动挂起，又不会引入明显唤醒开销。 */
const SLEEP_SLICE_MILLIS = 1_000;

/** 调度单次重读前直接检查文档，覆盖可见性监听尚未安装的首屏。 */
export function documentIsVisible(): boolean {
  return typeof document === "undefined" || !document.hidden;
}

interface VisibilityListener {
  readonly document: Document;
  readonly callback: () => void;
}

interface VisibilityState {
  hidden: boolean;
  readonly waiters: Set<() => void>;
  listener: VisibilityListener | null;
}

/** 页面可见性；克隆体与原体共享同一份可见性和等待者。 */
export interface PageVisibility {
  /** 克隆一个共享状态的句柄；同一页面的多个循环由此看到相同可见性。 */
  clone(): PageVisibility;
  /** 同步读取可见性；不建立额外监听或定时器。 */
  isVisible(): boolean;
  /** 安装 `visibilitychange` 监听；恢复可见时先执行 `onVisible`，再唤醒等待者。 */
  install(onVisible: () => void): void;
  /** 卸载 `visibilitychange` 监听，避免路由反复挂载时监听器累积。 */
  cleanup(): void;
  /** 页面隐藏时挂起直到恢复可见；`true` 表示已可见，`false` 表示等待被取消。 */
  waitUntilVisible(signal?: AbortSignal): Promise<boolean>;
}

/** 唤醒全部等待者；先清空登记表，再执行可能重新登记的回调。 */
function wakeWaiters(state: VisibilityState): void {
  const waiters = [...state.waiters];
  state.waiters.clear();
  for (const waiter of waiters) {
    waiter();
  }
}

/** 由一个共享状态构造句柄；句柄之间不复制可见性。 */
function visibilityHandle(state: VisibilityState): PageVisibility {
  return {
    clone() {
      return visibilityHandle(state);
    },
    isVisible() {
      return !state.hidden;
    },
    install(onVisible) {
      if (state.listener !== null || typeof document === "undefined") {
        return;
      }
      // 安装前先同步一次当前状态：监听器登记前页面可能已经是隐藏的。
      state.hidden = document.hidden;
      const callback = () => {
        const hidden = document.hidden;
        state.hidden = hidden;
        if (!hidden) {
          try {
            onVisible();
          } finally {
            wakeWaiters(state);
          }
        }
      };
      try {
        document.addEventListener("visibilitychange", callback);
      } catch {
        // 源码忽略注册失败但仍记录已安装；避免后续重复注册同一页面状态。
      }
      state.listener = { document, callback };
    },
    cleanup() {
      const listener = state.listener;
      if (listener === null) {
        return;
      }
      state.listener = null;
      try {
        listener.document.removeEventListener("visibilitychange", listener.callback);
      } catch {
        // 卸载失败不影响后续重新安装；监听器对象随句柄释放。
      }
    },
    async waitUntilVisible(signal) {
      // 唤醒到异步续体执行之间页面可能再次隐藏；每次醒来都重新检查并重新登记等待。
      while (state.hidden) {
        if (signal?.aborted === true) {
          return false;
        }
        const woke = await new Promise<boolean>((resolve) => {
          let settled = false;
          const finish = (visible: boolean) => {
            if (settled) {
              return;
            }
            settled = true;
            state.waiters.delete(wake);
            signal?.removeEventListener("abort", onAbort);
            resolve(visible);
          };
          const wake = () => finish(true);
          const onAbort = () => finish(false);
          state.waiters.add(wake);
          signal?.addEventListener("abort", onAbort, { once: true });
        });
        if (!woke) {
          return false;
        }
      }
      return true;
    },
  };
}

/** 创建一份页面可见性状态。 */
export function createPageVisibility(): PageVisibility {
  return visibilityHandle({ hidden: false, waiters: new Set(), listener: null });
}

/** 分片睡眠：隐藏时挂起剩余时长（不重置计时），恢复可见后继续走完剩余分片。 */
export async function sleepWhileVisible(
  visibility: PageVisibility,
  delayMillis: number,
  signal?: AbortSignal,
): Promise<void> {
  let remaining = Math.max(0, Math.trunc(delayMillis));
  while (remaining > 0) {
    if (!(await visibility.waitUntilVisible(signal))) {
      return;
    }
    // 续体执行前同步复查：此刻仍隐藏就重新等待，不扣减剩余时长。
    if (!visibility.isVisible()) {
      continue;
    }
    const slice = Math.min(remaining, SLEEP_SLICE_MILLIS);
    if (!(await sleep(slice, signal))) {
      return;
    }
    remaining -= slice;
  }
  // 最后一片计时期间也可能转入隐藏；不能在隐藏页完成等待并立即发起轮询或重连。
  for (;;) {
    if (!(await visibility.waitUntilVisible(signal))) {
      return;
    }
    if (visibility.isVisible()) {
      return;
    }
  }
}
