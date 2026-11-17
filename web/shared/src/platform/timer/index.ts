// 一次性定时原语：到时完成；提前取消会清除底层定时器，不留下迟到的回调。

/** 浏览器 `setTimeout` 能可靠表达的最大延时（32 位有符号上限）。 */
const MAX_TIMEOUT_MILLIS = 2_147_483_647;

/** 把调用方延时限制到浏览器可表达的范围；负值与非法值按 0 处理。 */
function boundedDelay(milliseconds: number): number {
  if (!Number.isFinite(milliseconds)) {
    return milliseconds === Number.POSITIVE_INFINITY ? MAX_TIMEOUT_MILLIS : 0;
  }
  return Math.min(Math.max(Math.trunc(milliseconds), 0), MAX_TIMEOUT_MILLIS);
}

/** 一次性定时等待句柄。 */
export interface TimeoutFuture {
  /** 到时完成；取消后保持挂起。 */
  readonly promise: Promise<void>;
  /** 取消底层定时器；重复调用无副作用。 */
  cancel(): void;
}

/** 创建一次性定时等待；超过浏览器上限的延时饱和到上限，保持“至少等这么久”。 */
export function createTimeoutFuture(milliseconds: number): TimeoutFuture {
  let finished = false;
  let resolveTimeout: (() => void) | null = null;
  const promise = new Promise<void>((resolve) => {
    resolveTimeout = resolve;
  });
  const handle = globalThis.setTimeout(() => {
    if (finished) {
      return;
    }
    finished = true;
    resolveTimeout?.();
  }, boundedDelay(milliseconds));
  return {
    promise,
    cancel() {
      if (finished) {
        return;
      }
      finished = true;
      globalThis.clearTimeout(handle);
    },
  };
}

/** 等待指定毫秒；到时返回 `true`，信号中止时清除定时器并返回 `false`。 */
export function sleep(milliseconds: number, signal?: AbortSignal): Promise<boolean> {
  if (signal?.aborted === true) {
    return Promise.resolve(false);
  }
  const timeout = createTimeoutFuture(milliseconds);
  return new Promise<boolean>((resolve) => {
    let settled = false;
    const finish = (completed: boolean) => {
      if (settled) {
        return;
      }
      settled = true;
      signal?.removeEventListener("abort", onAbort);
      if (!completed) {
        timeout.cancel();
      }
      resolve(completed);
    };
    const onAbort = () => finish(false);
    signal?.addEventListener("abort", onAbort, { once: true });
    void timeout.promise.then(() => finish(true));
  });
}
