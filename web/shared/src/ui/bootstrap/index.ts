// Public 与 Admin 共用的首屏启动门禁与浏览器错误信标。

import { createEffect } from "solid-js";

import { createValueMemo } from "../../reactive/memo";

const BROWSER_ERROR_ROUTE = "/api/v1/browser-errors";

/** WASM 恐慌、DTO 解码、实时协议与译文格式化共用的服务端闭集。 */
export type BrowserErrorClass = "wasm_panic" | "dto_decode" | "realtime_protocol" | "i18n_format";

/** 构建期注入的构建标识；非法时回退到包版本。 */
const INJECTED_BUILD_ID = __AUCTION_BUILD_ID__;
const BUILD_ID = buildIdIsValid(INJECTED_BUILD_ID)
  ? INJECTED_BUILD_ID
  : __AUCTION_PACKAGE_VERSION__;

let errorHookInstalled = false;
let panicReported = false;

export function browserErrorClassCode(value: BrowserErrorClass): string {
  return value;
}

export function buildId(): string {
  return BUILD_ID;
}

/** 构建标识：一至 64 字节的 ASCII 字母、数字、点、下划线或连字符。 */
export function buildIdIsValid(value: string): boolean {
  if (value.length < 1 || value.length > 64) {
    return false;
  }
  for (let index = 0; index < value.length; index += 1) {
    const code = value.charCodeAt(index);
    const allowed =
      (code >= 48 && code <= 57) ||
      (code >= 65 && code <= 90) ||
      (code >= 97 && code <= 122) ||
      code === 45 ||
      code === 46 ||
      code === 95;
    if (!allowed) {
      return false;
    }
  }
  return true;
}

/** 信标只带类别与构建标识；发送失败不能反过来影响页面。 */
export function reportBrowserError(errorClass: BrowserErrorClass): void {
  reportBrowserErrorWithBuildId(errorClass, BUILD_ID);
}

/** 首屏请求完成后才移除加载语义；宿主如果已被失败标记，就不再写就绪标记。
 * 参数必须是访问器，让条件从假变真时仍能触发写入；一次性布尔值会冻结在初始化时刻。 */
export function useAppBootReady(ready: () => boolean): void {
  const readyValue = createValueMemo(ready);
  createEffect(readyValue, (currentReady) => {
    if (!currentReady) {
      return;
    }
    if (typeof document === "undefined") {
      return;
    }
    const root = document.getElementById("main");
    if (root === null || root.hasAttribute("data-app-failed")) {
      return;
    }
    root.setAttribute("data-app-ready", "true");
    root.setAttribute("aria-busy", "false");
  });
}

/** 浏览器错误等价于 Rust 恐慌钩子：同一生命周期只上报一次，并保留可见失败标记。 */
export function installErrorHook(): void {
  if (errorHookInstalled) {
    return;
  }
  if (typeof window === "undefined") {
    return;
  }
  errorHookInstalled = true;
  window.addEventListener("error", (event) => {
    handleBrowserFailure(event.error ?? event.message);
  });
  window.addEventListener("unhandledrejection", (event) => {
    handleBrowserFailure(event.reason);
  });
}

/** 以显式构建标识发送信标；非法值降级为 `unknown`。导出仅供测试使用。 */
export function reportBrowserErrorWithBuildId(errorClass: BrowserErrorClass, value: string): void {
  if (typeof window === "undefined" || window.document === undefined) {
    return;
  }
  const reportedBuildId = buildIdIsValid(value) ? value : "unknown";
  const url = `${BROWSER_ERROR_ROUTE}?class=${browserErrorClassCode(errorClass)}&build=${reportedBuildId}`;
  try {
    void fetch(url, { method: "POST" }).catch(() => undefined);
  } catch {
    // fetch 可能同步抛出（宿主缺少 fetch 或实现被替换）；信标失败不能反过来影响页面或递归进失败钩子。
  }
}

/**
 * 应用主动上报不可恢复的失败（例如页面分块加载失败）。
 * 与未捕获异常走同一条路径：控制台输出、显示入口文档的错误横幅、只上报一次信标。
 */
export function reportAppFailure(value: unknown): void {
  handleBrowserFailure(value);
}

function handleBrowserFailure(value: unknown): void {
  console.error(`页面出现未处理的错误：${failureText(value)}`);
  showFailureBanner();
  if (!panicReported) {
    panicReported = true;
    reportBrowserError("wasm_panic");
  }
}

function showFailureBanner(): void {
  if (typeof document === "undefined") {
    return;
  }
  const root = document.getElementById("main");
  if (root === null) {
    return;
  }
  root.setAttribute("data-app-failed", "true");
  root.removeAttribute("data-app-ready");
  root.setAttribute("aria-busy", "false");
}

function failureText(value: unknown): string {
  try {
    return String(value);
  } catch {
    return "无法读取的错误";
  }
}
