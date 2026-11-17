// 原生 Web Locks 的有界、可取消独占作用域；不以标签页内互斥或浏览器存储伪造跨页锁。

import { createTimeoutFuture } from "../../platform/timer";
import { err, ok, type Result } from "../../result";

/** 原生锁尚未获取，请求尚未发送；不能把这类前置失败当作未知外写。 */
export type BrowserLockError = "unavailable" | "timeout";

/** 一个可由两个协作者分别等待与完成的 Promise。 */
function deferredPromise(): { readonly promise: Promise<void>; resolve(): void } {
  let resolvePromise: (() => void) | null = null;
  const promise = new Promise<void>((resolve) => {
    resolvePromise = resolve;
  });
  return {
    promise,
    resolve() {
      resolvePromise?.();
    },
  };
}

/**
 * 在原生独占锁内执行 `body`；只在安全上下文且浏览器提供 `navigator.locks` 时工作。
 * 排队阶段 `signal` 中止会放弃排队；持锁后它同时中止传给 `body` 的 `lockSignal`。
 * 锁始终在 `body` 收口后释放，正常返回、抛错或宿主提前失效都不会泄漏守卫。
 */
export async function withBrowserLock<T>(
  name: string,
  timeoutMillis: number,
  signal: AbortSignal | undefined,
  body: (lockSignal: AbortSignal) => Promise<T>,
): Promise<Result<T, BrowserLockError>> {
  if (typeof window === "undefined" || typeof navigator === "undefined") {
    return err("unavailable");
  }
  let secureContext = false;
  try {
    secureContext = window.isSecureContext === true;
  } catch {
    return err("unavailable");
  }
  if (!secureContext) {
    return err("unavailable");
  }

  let manager: LockManager | null = null;
  try {
    manager = navigator.locks ?? null;
  } catch {
    return err("unavailable");
  }
  if (manager === null) {
    return err("unavailable");
  }
  let requestAvailable = false;
  try {
    requestAvailable = typeof manager.request === "function";
  } catch {
    return err("unavailable");
  }
  if (!requestAvailable) {
    return err("unavailable");
  }

  let controller: AbortController;
  try {
    controller = new AbortController();
  } catch {
    return err("unavailable");
  }
  const grant = deferredPromise();
  const held = deferredPromise();
  let signalCancelled: (() => void) | null = null;
  const cancelled = new Promise<"cancelled">((resolve) => {
    signalCancelled = () => resolve("cancelled");
  });
  let abandoned = false;
  let released = false;
  const release = () => {
    if (released) {
      return;
    }
    released = true;
    // 已获取时不能 steal 其他标签页；必须完成 waiting Promise，让原生锁在 body 收口后释放。
    controller.abort();
    held.resolve();
  };
  const abortQueued = () => {
    signalCancelled?.();
    controller.abort();
  };
  signal?.addEventListener("abort", abortQueued, { once: true });
  if (signal?.aborted === true) {
    abortQueued();
  }
  const cleanupSignal = () => signal?.removeEventListener("abort", abortQueued);

  let request: Promise<void>;
  try {
    request = manager.request(
      name,
      { signal: controller.signal },
      (lock: Lock | null): Promise<void> => {
        if (lock === null || abandoned) {
          return Promise.resolve();
        }
        grant.resolve();
        // 原生锁持有到此 Promise 完成；请求完成或调用方取消后由 release() 释放。
        return held.promise;
      },
    );
  } catch {
    cleanupSignal();
    release();
    return err("unavailable");
  }
  const settled = request.then(
    () => "request_settled" as const,
    () => "request_settled" as const,
  );

  const timeout = createTimeoutFuture(timeoutMillis);
  const outcome = await Promise.race([
    grant.promise.then(() => "granted" as const),
    settled,
    timeout.promise.then(() => "timeout" as const),
    cancelled,
  ]);
  timeout.cancel();

  if (outcome !== "granted") {
    // 放弃排队或处理已排队的 grant：先标记录放弃，再中断排队，最后若已取得锁则完成 held。
    abandoned = true;
    cleanupSignal();
    release();
    return err(outcome === "timeout" ? "timeout" : "unavailable");
  }
  if (controller.signal.aborted) {
    // grant 与调用方中止同到：不进入 body，直接放弃已取得的锁。
    abandoned = true;
    cleanupSignal();
    release();
    return err("unavailable");
  }

  try {
    const value = await body(controller.signal);
    return ok(value);
  } finally {
    cleanupSignal();
    release();
  }
}
