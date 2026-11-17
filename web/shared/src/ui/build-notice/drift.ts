// 构建号漂移检测：只采信 `no-store` 响应上的 x-auction-build，首次偏差唤醒等待者。
// 纯逻辑与组件分离，避免测试运行器需要编译 Solid JSX。

import { buildId } from "../bootstrap";

/** 服务端报告构建号的响应头；与 auction-http 的契约保持一致。 */
export const SERVER_BUILD_HEADER = "x-auction-build";

/** 一次性的构建号偏差等待句柄。 */
export interface BuildDriftWait {
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

/** 仅供测试使用：读取模块级偏差状态；生产调用方经 `useBuildDrift` 订阅。 */
export function buildIsStale(): boolean {
  return staleState;
}

/** 仅供测试使用：恢复偏差状态并丢弃未决等待者。 */
export function resetBuildDriftForTests(): void {
  staleState = false;
  driftWaiters.clear();
}

/** 仅供测试使用：登记一个等待者；生产调用方经 `useBuildDrift` 创建。 */
export function waitForBuildDrift(): BuildDriftWait {
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
