// Markdown 行内语法解析与链接安全校验。

import { unreachable } from "../../../result";
import { utf8ByteLength } from "../../../text";
import type { Inline } from "./model";

/**
 * 行内解析递归深度上限：恶意或病态嵌套（如 `[[[[…]]]]`、交错强调标记）会逐层递归，
 * 超限后把剩余内容按纯文本处理，避免栈耗尽或指数级解析开销。
 */
const MAX_INLINE_DEPTH = 8;

/**
 * 单个链接最多接受 1 KiB 标签和 4 KiB 目标；超限按文本处理，不截断正文。
 * 记住下一个闭合符的位置，避免大量未闭合 `[` / `<http://` 重复扫描重叠窗口。
 */
const MAX_LINK_LABEL_BYTES = 1024;
const MAX_LINK_TARGET_BYTES = 4096;

const UNSAFE_TARGET_CHARACTER = /[\p{Cc}\\]/u;

type InlineKind = "strong" | "emphasis" | "strike";

interface DelimiterCache {
  position: number | null;
}

interface LinkDelimiters {
  readonly label: DelimiterCache;
  readonly target: DelimiterCache;
  readonly autolink: DelimiterCache;
}

interface BracketLink {
  readonly label: string;
  readonly target: string;
  readonly consumed: number;
}

/** 解析受支持的 Markdown 行内语法，并对链接目标执行白名单校验。 */
export function parseInline(value: string): Inline[] {
  return parseInlineAt(value, 0);
}

function parseInlineAt(value: string, depth: number): Inline[] {
  if (depth >= MAX_INLINE_DEPTH) {
    // 超限：不再解析任何行内语法，整段按纯文本返回。
    return [{ kind: "text", value }];
  }

  const nodes: Inline[] = [];
  const text = { value: "" };
  let index = 0;
  const delimiters: LinkDelimiters = {
    label: { position: null },
    target: { position: null },
    autolink: { position: null },
  };
  const emphasisClosers: DelimiterCache[] = Array.from({ length: 5 }, () => ({
    position: null,
  }));

  while (index < value.length) {
    const rest = value.slice(index);
    if (rest.startsWith("\\")) {
      const character = firstCharacter(rest.slice(1));
      if (character !== null) {
        text.value += character;
        index += 1 + character.length;
        continue;
      }
    }

    const image = bracketLink(value, index, true, delimiters);
    if (image !== null) {
      const source = safeLinkTarget(image.target);
      if (source !== null) {
        flushText(nodes, text);
        nodes.push({ kind: "image", alt: image.label, source });
        index += image.consumed;
        continue;
      }
    }

    const link = bracketLink(value, index, false, delimiters);
    if (link !== null) {
      const target = safeLinkTarget(link.target);
      if (target !== null) {
        flushText(nodes, text);
        nodes.push({
          kind: "link",
          label: parseInlineAt(link.label, depth + 1),
          target,
        });
        index += link.consumed;
        continue;
      }
    }

    let handled = false;
    const markers: readonly (readonly [string, InlineKind])[] = [
      ["**", "strong"],
      ["__", "strong"],
      ["~~", "strike"],
      ["*", "emphasis"],
      ["_", "emphasis"],
    ];
    for (let markerIndex = 0; markerIndex < markers.length; markerIndex += 1) {
      const marker = markers[markerIndex];
      const cache = emphasisClosers[markerIndex];
      if (marker === undefined || cache === undefined) {
        continue;
      }
      const [delimiter, kind] = marker;
      const innerStart = index + delimiter.length;
      if (!rest.startsWith(delimiter)) {
        continue;
      }
      const close = nextDelimiter(value, innerStart, delimiter, cache);
      if (close !== null && close > innerStart) {
        flushText(nodes, text);
        nodes.push(buildInline(kind, parseInlineAt(value.slice(innerStart, close), depth + 1)));
        index = close + delimiter.length;
        handled = true;
        break;
      }
    }
    if (handled) {
      continue;
    }

    if (rest.startsWith("`")) {
      const close = rest.slice(1).indexOf("`");
      if (close >= 0) {
        flushText(nodes, text);
        nodes.push({ kind: "code", value: rustTrim(rest.slice(1, close + 1)) });
        index += close + 2;
        continue;
      }
    }

    if (rest.startsWith("<http://") || rest.startsWith("<https://")) {
      const close = nextDelimiter(value, index + 1, ">", delimiters.autolink);
      if (close !== null && byteLengthWithinLimit(value, index + 1, close, MAX_LINK_TARGET_BYTES)) {
        const targetText = value.slice(index + 1, close);
        const target = safeLinkTarget(targetText);
        if (target !== null) {
          flushText(nodes, text);
          nodes.push({ kind: "link", label: [{ kind: "text", value: targetText }], target });
          index = close + 1;
          continue;
        }
      }
    }

    const character = firstCharacter(rest) ?? "";
    text.value += character === "\n" ? " " : character;
    index += character.length;
  }
  flushText(nodes, text);
  return nodes;
}

function buildInline(kind: InlineKind, nodes: readonly Inline[]): Inline {
  switch (kind) {
    case "strong":
      return { kind: "strong", nodes };
    case "emphasis":
      return { kind: "emphasis", nodes };
    case "strike":
      return { kind: "strike", nodes };
    default:
      return unreachable(kind);
  }
}

function bracketLink(
  value: string,
  start: number,
  image: boolean,
  delimiters: LinkDelimiters,
): BracketLink | null {
  const rest = value.slice(start);
  const prefixLength = image ? 2 : 1;
  if (image) {
    if (!rest.startsWith("![")) {
      return null;
    }
  } else if (!rest.startsWith("[")) {
    return null;
  }
  const labelStart = start + prefixLength;
  const closeLabel = nextDelimiter(value, labelStart, "](", delimiters.label);
  if (
    closeLabel === null ||
    !byteLengthWithinLimit(value, labelStart, closeLabel, MAX_LINK_LABEL_BYTES)
  ) {
    return null;
  }
  const targetStart = closeLabel + 2;
  const closeTarget = nextDelimiter(value, targetStart, ")", delimiters.target);
  if (
    closeTarget === null ||
    !byteLengthWithinLimit(value, targetStart, closeTarget, MAX_LINK_TARGET_BYTES)
  ) {
    return null;
  }
  return {
    label: value.slice(labelStart, closeLabel),
    target: value.slice(targetStart, closeTarget),
    consumed: closeTarget + 1 - start,
  };
}

/**
 * 先用码元距离排除必然超限的候选：每个码元至少编码为一个 UTF-8 字节，
 * 只有窗口本身不超过上限时才编码有界片段，避免未闭合链接造成平方级重复扫描。
 */
function byteLengthWithinLimit(value: string, start: number, end: number, limit: number): boolean {
  return end - start <= limit && utf8ByteLength(value.slice(start, end)) <= limit;
}

/** 主循环只向前移动；缓存跳过已确认的转义候选，缺失时以字符串末尾作为哨兵。 */
function nextDelimiter(
  value: string,
  start: number,
  delimiter: string,
  cached: DelimiterCache,
): number | null {
  let position = cached.position;
  if (position === null || position < start) {
    position = value.indexOf(delimiter, start);
  }
  while (position >= 0 && isEscapedAt(value, position)) {
    position = value.indexOf(delimiter, position + 1);
  }
  cached.position = position < 0 ? value.length : position;
  return position < 0 ? null : position;
}

/** 位置前的连续反斜杠为奇数时该字符被转义，不能作为分隔符。 */
function isEscapedAt(value: string, index: number): boolean {
  let backslashes = 0;
  for (let cursor = index - 1; cursor >= 0 && value.charAt(cursor) === "\\"; cursor -= 1) {
    backslashes += 1;
  }
  return backslashes % 2 === 1;
}

function safeLinkTarget(value: string): string | null {
  // 浏览器会把反斜杠当作斜杠，并移除 URL 内的 Tab/换行；必须在 trim 前拒绝原始目标。
  if (hasUnsafeTargetCharacter(value)) {
    return null;
  }
  const trimmed = rustTrim(value);
  const allowed =
    asciiPrefixEqualsIgnoreCase(trimmed, "https://") ||
    asciiPrefixEqualsIgnoreCase(trimmed, "http://") ||
    asciiPrefixEqualsIgnoreCase(trimmed, "mailto:") ||
    isInternalTarget(trimmed);
  return allowed ? trimmed : null;
}

/** 与目标白名单共用站内判断；任何反斜杠或控制字符都不能成为站内导航。 */
export function isInternalTarget(target: string): boolean {
  return (
    !hasUnsafeTargetCharacter(target) &&
    ((target.startsWith("/") && !target.startsWith("//")) ||
      target.startsWith("./") ||
      target.startsWith("../") ||
      target.startsWith("#") ||
      target.startsWith("?"))
  );
}

function hasUnsafeTargetCharacter(target: string): boolean {
  return UNSAFE_TARGET_CHARACTER.test(target);
}

function asciiPrefixEqualsIgnoreCase(value: string, prefix: string): boolean {
  if (value.length < prefix.length) {
    return false;
  }
  for (let index = 0; index < prefix.length; index += 1) {
    let left = value.charCodeAt(index);
    let right = prefix.charCodeAt(index);
    if (left >= 65 && left <= 90) {
      left += 32;
    }
    if (right >= 65 && right <= 90) {
      right += 32;
    }
    if (left !== right) {
      return false;
    }
  }
  return true;
}

function firstCharacter(value: string): string | null {
  const codePoint = value.codePointAt(0);
  return codePoint === undefined ? null : String.fromCodePoint(codePoint);
}

function flushText(nodes: Inline[], text: { value: string }): void {
  if (text.value.length > 0) {
    nodes.push({ kind: "text", value: text.value });
    text.value = "";
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
