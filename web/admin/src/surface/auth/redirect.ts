// 登录跳转与站内 return 地址。

import { parseQuerySearch } from "@auction/web-shared/net/query";
import { useRouter } from "@auction/web-shared/router";
import { compileRoutes, resolveRoute } from "@auction/web-shared/router/pattern";
import type { JSX } from "@solidjs/web";
import { createEffect } from "solid-js";

import { ADMIN_ROUTES } from "../router/route";

const ADMIN_COMPILED_ROUTES = compileRoutes(ADMIN_ROUTES);

/** 未认证访问受保护页面时，按旧版保存站内 return 地址并替换到登录页。 */
export function AdminLoginRedirect(): JSX.Element {
  const router = useRouter();
  const returnTarget = currentAdminLocation();
  const encodedReturn = formUrlencodedByteSerialize(returnTarget);
  const target = `/login?return=${encodedReturn}`;
  createEffect(
    () => null,
    () => {
      router.replace(target);
    },
  );
  return null;
}

/** 已认证用户访问登录路径时，按旧版 return 参数回到原始站内地址。 */
export function AdminReturnRedirect(): JSX.Element {
  const router = useRouter();
  const target = adminReturnPath();
  createEffect(
    () => null,
    () => {
      // 共享路由的 replace 保留当前历史条目键并同步匹配；它没有“原样地址 + 页首落点”
      // 的公开入口，尾斜杠、裸问号与片段定位的差异见迁移报告。
      router.replace(target);
    },
  );
  return null;
}

function currentAdminLocation(): string {
  if (typeof window === "undefined") {
    return "/";
  }
  let pathname: string;
  try {
    pathname = window.location.pathname;
  } catch {
    pathname = "/";
  }
  let search: string;
  try {
    search = window.location.search;
  } catch {
    search = "";
  }
  let hash: string;
  try {
    hash = window.location.hash;
  } catch {
    hash = "";
  }
  if (pathname.startsWith("/") && pathname !== "/login") {
    return `${pathname}${search}${hash}`;
  }
  return "/";
}

function adminReturnPath(): string {
  if (typeof window === "undefined") {
    return "/";
  }
  let originText: string;
  try {
    originText = window.location.origin;
  } catch {
    return "/";
  }
  let origin: URL;
  try {
    origin = new URL(originText);
  } catch {
    return "/";
  }
  let search: string;
  try {
    search = window.location.search;
  } catch {
    search = "";
  }
  return returnPathForQuery(search, origin);
}

/**
 * 复现 Rust Router 的冷启动路径规范化。
 *
 * Rust 先丢掉地址中的查询与片段，再按规范化后的路径判断是否补回；只有路径相同或原地址
 * 仅多一个尾斜杠时才恢复。共享路由器会原样保留后缀，因此对会被 Rust 丢掉的地址，在
 * Router 规范化地址前把地址改成 Rust 初始化历史后的形状。
 */
export function normalizeColdStartAdminLocation(): void {
  if (typeof window === "undefined") {
    return;
  }
  try {
    const location = window.location;
    const { pathname, search, hash } = location;
    // 路径不能安全作为站内地址时不改写浏览器历史。
    if (!pathname.startsWith("/") || pathname.startsWith("//") || pathname.includes("\\")) {
      return;
    }
    const resolved = resolveRoute(ADMIN_COMPILED_ROUTES, pathname);
    const normalized = resolved.routeString;
    const trailingSlashDropped = pathname.endsWith("/") ? pathname.slice(0, -1) : null;
    const pathPreserved = normalized === pathname || normalized === trailingSlashDropped;
    // Rust 初始化历史时用 pathname + search + hash 重写地址：裸 `?`/`#` 的属性为空，
    // 因此空后缀也会丢掉；路径被规范化时后缀恢复条件不成立，后缀同样丢弃。
    // `/setup` 在认证外壳之外，Rust 不会执行认证前的后缀恢复，冷启动直接停在 `/setup`。
    const target =
      resolved.match.kind === "matched" && resolved.match.name === "Setup"
        ? normalized
        : pathPreserved
          ? `${normalized}${search}${hash}`
          : normalized;
    if (currentPathAndSuffix(location.href, location.origin) === target) {
      return;
    }
    window.history.replaceState(window.history.state, "", target);
  } catch {
    // 地址读取或写入失败时保持原样；后续认证与回跳仍按当前地址处理。
  }
}

/** 当前地址的路径与后缀；`URL` 的 `search`/`hash` 看不到裸 `?`/`#`，必须从 `href` 还原。 */
function currentPathAndSuffix(href: string, origin: string): string {
  return href.startsWith(origin) ? href.slice(origin.length) : href;
}

/** 只接受同源、未消解点段、没有被编码分隔符隐藏的站内地址；登录路由会造成自循环。 */
function returnPathForQuery(search: string, origin: URL): string {
  // Rust 只剥第一个 `?`；把完整 search 交给共享解析器，多出的问号属于参数名。
  if (!search.startsWith("?")) {
    return "/";
  }
  let value: string | null = null;
  for (const [key, candidate] of parseQuerySearch(search)) {
    if (key === "return") {
      value = candidate;
      break;
    }
  }
  if (value === null) {
    return "/";
  }
  if (
    !value.startsWith("/") ||
    value.startsWith("//") ||
    value.includes("\\") ||
    hasControlCharacter(value)
  ) {
    return "/";
  }
  let normalized: URL;
  try {
    normalized = new URL(value, origin);
  } catch {
    return "/";
  }
  const path = normalized.pathname;
  // 浏览器会消解点段；路径中另拒绝重复斜线与编码分隔符，避免路由和 URL 对段边界解释不同。
  if (
    normalized.origin !== origin.origin ||
    !matchesRustSerializedPathAndSuffix(normalized, value) ||
    path.includes("//") ||
    hasEncodedPathSeparator(path)
  ) {
    return "/";
  }
  const resolved = resolveRoute(ADMIN_COMPILED_ROUTES, path);
  // Rust 的 `Route` 含 catch-all，未知路径也会解析为 `NotFound` 而不是失败；只拒绝 Login。
  if (resolved.match.kind === "matched" && resolved.match.name === "Login") {
    return "/";
  }
  return value;
}

/**
 * 比较浏览器规范化后的路径后缀与原始值。
 *
 * Rust `url` 2.5.8 的路径转义集合不含 `^` 与 `|`，而浏览器会把它们写成 `%5E`/`%7C`；
 * 这两个字符不是路径分隔符，比较前还原成字面量，避免把两端都接受的地址判成改写。
 */
function matchesRustSerializedPathAndSuffix(url: URL, value: string): boolean {
  return rustPathPunctuation(serializedPathAndSuffix(url)) === rustPathPunctuation(value);
}

function rustPathPunctuation(text: string): string {
  const suffixStart = text.search(/[?#]/u);
  const path = suffixStart === -1 ? text : text.slice(0, suffixStart);
  const suffix = suffixStart === -1 ? "" : text.slice(suffixStart);
  return `${path.replace(/%5e/giu, "^").replace(/%7c/giu, "|")}${suffix}`;
}

/** 取路径及其后原文；空的 `?`/`#` 在 `URL` 的 `search`/`hash` 属性里不可见，必须从序列化地址还原。 */
function serializedPathAndSuffix(url: URL): string {
  return url.href.startsWith(url.origin) ? url.href.slice(url.origin.length) : url.href;
}

function hasControlCharacter(value: string): boolean {
  return /\p{Cc}/u.test(value);
}

function hasEncodedPathSeparator(path: string): boolean {
  return /%2f|%5c/iu.test(path);
}

/** 按 `form_urlencoded::byte_serialize` 编码；空格写成 `+`，其余非保留字节用大写百分号转义。 */
function formUrlencodedByteSerialize(value: string): string {
  const bytes = new TextEncoder().encode(value);
  let encoded = "";
  for (const byte of bytes) {
    if (byte === 0x20) {
      encoded += "+";
      continue;
    }
    if (isFormUrlencodedByte(byte)) {
      encoded += String.fromCharCode(byte);
      continue;
    }
    encoded += `%${byte.toString(16).toUpperCase().padStart(2, "0")}`;
  }
  return encoded;
}

function isFormUrlencodedByte(byte: number): boolean {
  return (
    (byte >= 0x30 && byte <= 0x39) ||
    (byte >= 0x41 && byte <= 0x5a) ||
    (byte >= 0x61 && byte <= 0x7a) ||
    byte === 0x2a ||
    byte === 0x2d ||
    byte === 0x2e ||
    byte === 0x5f
  );
}
