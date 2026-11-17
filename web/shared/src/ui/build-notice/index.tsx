// Public 与 Admin 共用的服务端构建漂移检测与刷新提示。
// 只采信 `no-store` 响应上的 x-auction-build；与自身构建号不一致时提示刷新，
// 不自动重载正在操作的页面。

import type { JSX } from "@solidjs/web";
import { type Accessor, onCleanup, Show } from "solid-js";
import { createCell } from "../../reactive/cell";
import { buildId } from "../bootstrap";

/** 服务端报告构建号的响应头；与 auction-http 的契约保持一致。 */
export const SERVER_BUILD_HEADER = "x-auction-build";

interface BuildDriftWait {
  readonly promise: Promise<void>;
  cancel(): void;
}

interface DriftWaiter {
  readonly id: number;
  resolve(): void;
}

let nextWaiterId = 0;
let staleState = false;
const driftWaiters = new Set<DriftWaiter>();

/** 只有响应明确禁止缓存时，服务端构建标识才代表当前页面实际加载的前端资源。 */
export function responseReportsCurrentBuild(cacheControl: string | null): boolean {
  if (cacheControl === null) {
    return false;
  }
  for (const directive of cacheControl.split(",")) {
    if (asciiEqualsIgnoreCase(rustTrim(directive), "no-store")) {
      return true;
    }
  }
  return false;
}

/** 比较服务端报告值；返回是否与自身构建号不一致。相同标识走无锁快路径。 */
export function observeServerBuildId(reported: string): boolean {
  if (reported === buildId()) {
    return false;
  }
  markStale();
  return true;
}

function markStale(): void {
  if (staleState) {
    return;
  }
  staleState = true;
  const waiters = Array.from(driftWaiters);
  driftWaiters.clear();
  for (const waiter of waiters) {
    waiter.resolve();
  }
}

function buildIsStale(): boolean {
  return staleState;
}

/** 订阅构建号偏差；已偏差时立即为 true，之后保持到页面刷新。 */
export function useBuildDrift(): Accessor<boolean> {
  const stale = createCell(buildIsStale());
  if (stale.peek()) {
    return stale;
  }
  const wait = waitForBuildDrift();
  let active = true;
  // 等待者在组件初始化时已经登记，清理也必须同时建立；只放到 onSettled 会漏掉此前卸载。
  onCleanup(() => {
    active = false;
    wait.cancel();
  });
  void wait.promise.then(() => {
    if (active) {
      stale.set(true);
    }
  });
  return stale;
}

/** 部署后旧页面的刷新提示；文案由调用方提供，不使用浏览器自动刷新。 */
export function StaleBuildBanner(props: { message: string; refreshLabel: string }): JSX.Element {
  const stale = useBuildDrift();
  return (
    <Show when={stale()}>
      <div class="shared-stale-build" role="status">
        <p class="shared-stale-build__message">{props.message}</p>
        <button type="button" class="shared-stale-build__refresh" onClick={() => reloadPage()}>
          {props.refreshLabel}
        </button>
      </div>
    </Show>
  );
}

function waitForBuildDrift(): BuildDriftWait {
  if (buildIsStale()) {
    return { promise: Promise.resolve(), cancel: () => undefined };
  }
  let resolveWaiter: (() => void) | null = null;
  const promise = new Promise<void>((resolve) => {
    resolveWaiter = resolve;
  });
  const waiter: DriftWaiter = {
    id: nextWaiterId,
    resolve() {
      const current = resolveWaiter;
      resolveWaiter = null;
      current?.();
    },
  };
  nextWaiterId += 1;
  driftWaiters.add(waiter);
  return {
    promise,
    cancel() {
      driftWaiters.delete(waiter);
    },
  };
}

function reloadPage(): void {
  if (typeof window === "undefined") {
    return;
  }
  try {
    window.location.reload();
  } catch {
    // 浏览器拒绝重载时保持当前页面；下一次成功响应仍会再次提示。
  }
}

function rustTrim(value: string): string {
  let start = 0;
  while (start < value.length && isRustWhitespace(value.charCodeAt(start))) {
    start += 1;
  }
  let end = value.length;
  while (end > start && isRustWhitespace(value.charCodeAt(end - 1))) {
    end -= 1;
  }
  return value.slice(start, end);
}

function isRustWhitespace(code: number): boolean {
  return (
    (code >= 0x0009 && code <= 0x000d) ||
    code === 0x0020 ||
    code === 0x0085 ||
    code === 0x00a0 ||
    code === 0x1680 ||
    (code >= 0x2000 && code <= 0x200a) ||
    code === 0x2028 ||
    code === 0x2029 ||
    code === 0x202f ||
    code === 0x205f ||
    code === 0x3000
  );
}

function asciiEqualsIgnoreCase(left: string, right: string): boolean {
  if (left.length !== right.length) {
    return false;
  }
  for (let index = 0; index < left.length; index += 1) {
    let leftCode = left.charCodeAt(index);
    let rightCode = right.charCodeAt(index);
    if (leftCode >= 65 && leftCode <= 90) {
      leftCode += 32;
    }
    if (rightCode >= 65 && rightCode <= 90) {
      rightCode += 32;
    }
    if (leftCode !== rightCode) {
      return false;
    }
  }
  return true;
}
