// 路由定义、路径匹配与规范路由串：只处理静态段、单个动态段和已声明查询参数。
// 路径匹配沿用 Dioxus 0.7；地址规范化只改路径，不重写页面拥有的查询与片段。

import { parseQuerySearch } from "../net/query";

/**
 * 一条路由声明；query 列出由路由解码并交给页面的参数。
 *
 * 根路径 `/` 除外，其余 pattern 只允许末尾的一个斜杠产生空段；中间空段会在编译路由表时拒绝。
 * 任意两条定义不得匹配同一地址：段数相同，且每个位置要么是相同的静态段，
 * 要么至少一边是参数段，即视为歧义并在编译时拒绝。这样匹配结果与声明顺序无关。
 */
export interface RouteDefinition {
  readonly name: string;
  readonly pattern: string;
  readonly query?: readonly string[];
}

/** 已解码的路径参数。 */
export type RouteParams = Readonly<Record<string, string>>;

/** 路由声明中存在的查询参数；缺失的参数不写入对象。 */
export type RouteQuery = Readonly<Record<string, string>>;

/** 命中路由定义后的结果。 */
export interface MatchedRoute {
  readonly kind: "matched";
  readonly name: string;
  readonly params: RouteParams;
  readonly query: RouteQuery;
}

/** 未命中任何定义时的结果；segments 是去掉前导斜杠后逐段解码的路径。 */
export interface NotFoundRoute {
  readonly kind: "not-found";
  readonly segments: readonly string[];
}

/** 当前路由匹配结果。 */
export type RouteMatch = MatchedRoute | NotFoundRoute;

interface StaticPatternSegment {
  readonly kind: "static";
  readonly value: string;
}

interface DynamicPatternSegment {
  readonly kind: "dynamic";
  readonly name: string;
}

type PatternSegment = StaticPatternSegment | DynamicPatternSegment;

/** 编译后的路由定义；仅供路由模块内部使用。 */
export interface CompiledRoute {
  readonly definition: RouteDefinition;
  readonly segments: readonly PatternSegment[];
}

/** 一次地址解析得到的匹配与规范串。 */
export interface ResolvedRoute {
  readonly match: RouteMatch;
  readonly routeString: string;
}

const HEX_DIGITS = "0123456789ABCDEF";
const PATH_EXTRA_BYTES = new Set(['"', "#", "%", "/", "<", ">", "?", "\\", "^", "`", "{", "}"]);
// ignoreBOM 必须为 true：Rust 的 String::from_utf8 保留 BOM，而 TextDecoder 默认会吞掉它。
const UTF8_DECODER = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true });

function isHexDigit(byte: number): boolean {
  return (
    (byte >= 0x30 && byte <= 0x39) ||
    (byte >= 0x41 && byte <= 0x46) ||
    (byte >= 0x61 && byte <= 0x66)
  );
}

function hexDigitValue(byte: number): number {
  if (byte >= 0x30 && byte <= 0x39) {
    return byte - 0x30;
  }
  if (byte >= 0x41 && byte <= 0x46) {
    return byte - 0x41 + 10;
  }
  return byte - 0x61 + 10;
}

/**
 * 按 percent_encoding::percent_decode_str(...).decode_utf8().unwrap_or(原文) 的规则解码。
 * 非法百分号序列保持字面量；解码后不是合法 UTF-8 时整段退回原文，不做部分解码。
 */
function percentDecode(value: string): string {
  if (!value.includes("%")) {
    return value;
  }
  const encoder = new TextEncoder();
  const bytes: number[] = [];
  let index = 0;
  while (index < value.length) {
    const current = value.charCodeAt(index);
    if (current === 0x25 && index + 2 < value.length) {
      const high = value.charCodeAt(index + 1);
      const low = value.charCodeAt(index + 2);
      if (isHexDigit(high) && isHexDigit(low)) {
        bytes.push((hexDigitValue(high) << 4) | hexDigitValue(low));
        index += 3;
        continue;
      }
    }
    const codePoint = value.codePointAt(index);
    if (codePoint === undefined) {
      break;
    }
    const character = String.fromCodePoint(codePoint);
    bytes.push(...encoder.encode(character));
    index += character.length;
  }
  try {
    return UTF8_DECODER.decode(Uint8Array.from(bytes));
  } catch {
    return value;
  }
}

function shouldEncodePathByte(byte: number): boolean {
  return (
    byte < 0x20 || byte === 0x7f || byte === 0x20 || PATH_EXTRA_BYTES.has(String.fromCharCode(byte))
  );
}

function percentEncode(value: string, shouldEncode: (byte: number) => boolean): string {
  const bytes = new TextEncoder().encode(value);
  let encoded = "";
  for (const byte of bytes) {
    if (byte < 0x80 && !shouldEncode(byte)) {
      encoded += String.fromCharCode(byte);
      continue;
    }
    encoded += `%${HEX_DIGITS[byte >> 4]}${HEX_DIGITS[byte & 0x0f]}`;
  }
  return encoded;
}

/** 编码一个路径段，段内的分隔字符和百分号不可在规范化时改变路由边界。 */
export function encodePathSegment(value: string): string {
  return percentEncode(value, shouldEncodePathByte);
}

function compilePattern(definition: RouteDefinition): CompiledRoute {
  const withoutHash = definition.pattern.split("#", 1)[0] ?? "";
  const withoutQuery = withoutHash.split("?", 1)[0] ?? "";
  const segments: PatternSegment[] = [];
  const patternSegments = withoutQuery.startsWith("/")
    ? withoutQuery.slice(1).split("/")
    : withoutQuery.split("/");
  // 根路径由空段数表达；其他 path 的末尾空段保留在格式化结果里，但匹配时会跳过。
  if (withoutQuery !== "/") {
    for (const segment of patternSegments) {
      if (segment.startsWith(":")) {
        segments.push({ kind: "dynamic", name: segment.slice(1) });
      } else {
        segments.push({ kind: "static", value: segment });
      }
    }
  }
  for (const [index, segment] of segments.entries()) {
    if (
      segment.kind === "static" &&
      segment.value === "" &&
      !isTrailingEmptySegment(segments, index)
    ) {
      throw new Error(
        `路由定义 "${definition.name}" 的 pattern "${definition.pattern}" 含中间空路径段；只允许末尾有一个斜杠`,
      );
    }
  }
  return { definition, segments };
}

function isTrailingEmptySegment(segments: readonly PatternSegment[], index: number): boolean {
  const segment = segments[index];
  return (
    segment !== undefined &&
    segment.kind === "static" &&
    segment.value === "" &&
    segments.length > 1 &&
    index === segments.length - 1
  );
}

function routeSegmentsWithoutTrailingSlash(route: CompiledRoute): readonly PatternSegment[] {
  return isTrailingEmptySegment(route.segments, route.segments.length - 1)
    ? route.segments.slice(0, -1)
    : route.segments;
}

function routesOverlap(left: CompiledRoute, right: CompiledRoute): boolean {
  const leftSegments = routeSegmentsWithoutTrailingSlash(left);
  const rightSegments = routeSegmentsWithoutTrailingSlash(right);
  if (leftSegments.length !== rightSegments.length) {
    return false;
  }
  return leftSegments.every((leftSegment, index) => {
    const rightSegment = rightSegments[index];
    if (rightSegment === undefined) {
      return false;
    }
    if (leftSegment.kind === "static" && rightSegment.kind === "static") {
      return leftSegment.value === rightSegment.value;
    }
    return true;
  });
}

/**
 * 编译路由表并拒绝会让同一地址命中多条定义的声明。
 * 不做 Dioxus 路由树的排序：拒绝歧义后，每个地址至多有一条定义匹配，结果不再依赖候选顺序。
 */
export function compileRoutes(routes: readonly RouteDefinition[]): readonly CompiledRoute[] {
  const compiled = routes.map(compilePattern);
  for (let leftIndex = 0; leftIndex < compiled.length; leftIndex += 1) {
    const left = compiled[leftIndex];
    if (left === undefined) {
      continue;
    }
    for (let rightIndex = leftIndex + 1; rightIndex < compiled.length; rightIndex += 1) {
      const right = compiled[rightIndex];
      if (right !== undefined && routesOverlap(left, right)) {
        throw new Error(
          `路由定义 "${left.definition.name}" (${left.definition.pattern}) 与 "${right.definition.name}" (${right.definition.pattern}) 会匹配同一地址；路由表不允许歧义定义`,
        );
      }
    }
  }
  return compiled;
}

interface ParsedHref {
  readonly segments: readonly string[];
  readonly search: string;
  readonly hash: string;
}

function parseHref(href: string): ParsedHref | null {
  const hashIndex = href.indexOf("#");
  const routeAndQuery = hashIndex === -1 ? href : href.slice(0, hashIndex);
  const queryIndex = routeAndQuery.indexOf("?");
  const pathname = queryIndex === -1 ? routeAndQuery : routeAndQuery.slice(0, queryIndex);
  const search = queryIndex === -1 ? "" : routeAndQuery.slice(queryIndex);
  const hash = hashIndex === -1 ? "" : href.slice(hashIndex);
  const path = pathname.endsWith("/") ? pathname.slice(0, -1) : pathname;
  if (!href.startsWith("/")) {
    return null;
  }
  const segments = path === "" ? [] : path.slice(1).split("/").map(percentDecode);
  return { segments, search: search === "?" ? "" : search, hash };
}

function matchCompiledRoute(
  route: CompiledRoute,
  segments: readonly string[],
  queryValues: ReadonlyMap<string, string>,
): MatchedRoute | null {
  const params: Record<string, string> = {};
  let pathIndex = 0;
  for (const [index, patternSegment] of route.segments.entries()) {
    // Dioxus 只跳过定义末尾的空静态段，因此 `/x/` 接受 `/x` 和 `/x/`，中间空段仍消费一个输入段。
    if (isTrailingEmptySegment(route.segments, index)) {
      continue;
    }
    const pathSegment = segments[pathIndex];
    if (pathSegment === undefined) {
      return null;
    }
    pathIndex += 1;
    if (patternSegment.kind === "static") {
      if (patternSegment.value !== pathSegment) {
        return null;
      }
      continue;
    }
    params[patternSegment.name] = pathSegment;
  }
  if (pathIndex !== segments.length) {
    return null;
  }
  const query: Record<string, string> = {};
  for (const name of route.definition.query ?? []) {
    const value = queryValues.get(name);
    if (value !== undefined) {
      query[name] = value;
    }
  }
  return { kind: "matched", name: route.definition.name, params, query };
}

function routePath(route: CompiledRoute, params: RouteParams): string {
  if (route.segments.length === 0) {
    return "/";
  }
  const path = route.segments
    .map((segment) => {
      if (segment.kind === "static") {
        return segment.value;
      }
      return encodePathSegment(params[segment.name] ?? "");
    })
    .join("/");
  return `/${path}`;
}

function notFoundRouteString(segments: readonly string[]): string {
  if (segments.length === 0) {
    return "/";
  }
  return `/${segments.map(encodePathSegment).join("/")}`;
}

/** 解析一个地址；未命中返回 not-found 与解码后的路径段。 */
export function resolveRoute(routes: readonly CompiledRoute[], href: string): ResolvedRoute {
  const parsed = parseHref(href);
  if (parsed === null) {
    const segments = href === "" ? [] : [href];
    return {
      match: { kind: "not-found", segments },
      routeString: notFoundRouteString(segments),
    };
  }
  // 分隔外层字段后再解码，return_to 内的分隔符属于该字段的值。
  const queryValues = new Map(parseQuerySearch(parsed.search));
  // 未声明的查询也可能由页面独立消费，且重编码可能改变字面键名（如 `?page`）。
  const suffix = `${parsed.search}${parsed.hash}`;
  for (const route of routes) {
    const match = matchCompiledRoute(route, parsed.segments, queryValues);
    if (match !== null) {
      return { match, routeString: `${routePath(route, match.params)}${suffix}` };
    }
  }
  return {
    match: { kind: "not-found", segments: parsed.segments },
    routeString: `${notFoundRouteString(parsed.segments)}${suffix}`,
  };
}

/**
 * 判断链接是否由本站路由处理。
 * 两张现有路由表都有末尾 catch-all，因此任意以 `/` 开头的地址都能被 Dioxus 路由解析，
 * 包括协议相对地址；未命中的路径由 not-found 页面处理。绝对 URL、片段与相对地址交给浏览器。
 */
export function isInternalHref(href: string): boolean {
  return href.startsWith("/");
}
