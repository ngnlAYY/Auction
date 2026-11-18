// 登录返回地址与需要认证入口的链接生成。
// 首次同源地址在 Router 规范化前保存；登录 key 不经过这里，也不进入任何地址状态。

import { useRouter } from "@auction/web-shared/router";
import { createContext, useContext } from "solid-js";

import { readBrowserLocationRevision } from "../query/location";
import { queryPairsString } from "../query/serialization";

/** 在 Router 规范化地址前保存首次同源 pathname + search；与 Rust 上下文读取一样可重复使用。 */
export class InitialLoginReturnLocation {
  readonly value: string;

  constructor(value: string) {
    this.value = value;
  }
}

export function initialLoginReturnLocation(): InitialLoginReturnLocation {
  const value = browserPathAndSearch() ?? "/";
  return new InitialLoginReturnLocation(safeReturnPath(value));
}

export const InitialLoginReturnLocationContext = createContext<InitialLoginReturnLocation | null>(
  null,
);

export function rustTrim(value: string): string {
  let start = 0;
  let end = value.length;
  while (start < end && isRustWhitespace(value.charCodeAt(start))) {
    start += 1;
  }
  while (end > start && isRustWhitespace(value.charCodeAt(end - 1))) {
    end -= 1;
  }
  return value.slice(start, end);
}

function isRustWhitespace(code: number): boolean {
  return (
    (code >= 0x09 && code <= 0x0d) ||
    code === 0x20 ||
    code === 0x85 ||
    code === 0xa0 ||
    code === 0x1680 ||
    (code >= 0x2000 && code <= 0x200a) ||
    code === 0x2028 ||
    code === 0x2029 ||
    code === 0x202f ||
    code === 0x205f ||
    code === 0x3000
  );
}

function hasAsciiControl(value: string): boolean {
  for (const character of value) {
    const code = character.codePointAt(0) ?? 0;
    if (code <= 0x1f || code === 0x7f) {
      return true;
    }
  }
  return false;
}

export function safeReturnPath(value: string | null): string {
  if (value === null) {
    return "/";
  }
  if (
    !value.startsWith("/") ||
    value.startsWith("//") ||
    value.startsWith("/login?") ||
    value === "/login" ||
    value.includes("\\") ||
    value.includes("#") ||
    hasAsciiControl(value)
  ) {
    return "/";
  }
  return value;
}

/**
 * 导航里“登录”链接的地址。与 Rust 版 `Route::Login { return_to: None }` 的显示串一致：
 * Dioxus 对“已声明但为空”的查询输出裸 `?`。它与地址栏的 `/login` 不相等，所以这条链接在
 * 登录页不会被标成当前页。顶栏会话菜单与“会话已在别处结束”提示里的登录入口在 Rust 版里
 * 直接写的是 `/login`，它们在登录页会被标成当前页；两者不要混用。
 */
export const LOGIN_ROUTE_HREF = "/login?";

/** 为需要认证的 CTA 保存当前 pathname 与 search；直接登录入口不携带返回地址。
 *
 * 源码只在渲染链接属性时调用本函数；事件处理器不得调用，因为 `useContext` 需要组件宿主。
 * 初始地址上下文在 Router 启动后失效，本函数只读取它，不在这里销毁。链接节点会被复用，
 * 直接读地址栏不会建立依赖；先读路由位置与直接 History 写入的版本，让 href 随同路径查询
 * 导航和查询状态写回重算。
 */
export function loginHrefForCurrentLocation(): string {
  const router = useRouter();
  router.currentLocationString();
  readBrowserLocationRevision();
  const initial = useContext(InitialLoginReturnLocationContext);
  const returnTo = currentPathAndSearch(initial);
  if (returnTo === null) {
    return "/login";
  }
  // 旧版用 Router state 保存来源；根路径本就是登录页默认返回值，地址栏保持裸 `/login`。
  if (returnTo === "/") {
    return "/login";
  }
  return `/login?${queryPairsString([["return_to", returnTo]])}`;
}

function currentPathAndSearch(initial: InitialLoginReturnLocation | null): string | null {
  const current = browserPathAndSearch();
  if (current === null) {
    return null;
  }
  if (current.includes("?")) {
    return safeReturnPath(current) === current ? current : null;
  }
  const initialValue = initial?.value ?? null;
  if (
    initialValue !== null &&
    (initialValue.split("?")[0] ?? "") === current &&
    safeReturnPath(initialValue) === initialValue
  ) {
    return initialValue;
  }
  return safeReturnPath(current) === current ? current : null;
}

function browserPathAndSearch(): string | null {
  if (typeof window === "undefined") {
    return null;
  }
  let pathname: string;
  try {
    pathname = window.location.pathname;
  } catch {
    return null;
  }
  let search: string;
  try {
    search = window.location.search;
  } catch {
    search = "";
  }
  return `${pathname}${search}`;
}
