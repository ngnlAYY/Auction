// Markdown 块级语法解析。

import { utf8ByteLength } from "../../../text";
import { parseInline } from "./inline";
import type { Inline, MarkdownBlock } from "./model";

const MAX_USIZE = 0xffff_ffff;

interface FenceStart {
  readonly fence: string;
  readonly language: string | null;
}

interface OrderedItem {
  readonly start: number;
  readonly value: string;
}

/** 把 Markdown 解析为受支持的块级节点；不执行原始 HTML。 */
export function parseBlocks(markdown: string): MarkdownBlock[] {
  const source = markdown.startsWith("\uFEFF") ? markdown.slice(1) : markdown;
  const lines = rustLines(source);
  const blocks: MarkdownBlock[] = [];
  let index = 0;

  while (index < lines.length) {
    const line = lines[index];
    if (line === undefined) {
      break;
    }
    const trimmed = rustTrim(line);
    if (trimmed.length === 0) {
      index += 1;
      continue;
    }

    const fence = fenceStart(line);
    if (fence !== null) {
      const value: string[] = [];
      index += 1;
      while (index < lines.length) {
        const candidateLine = lines[index];
        if (candidateLine === undefined) {
          break;
        }
        const candidate = rustTrimStart(candidateLine);
        // 结束围栏必须与开头同类且至少同长；短标记或带正文的行仍属于代码。
        if (isClosingFence(candidate, fence.fence)) {
          index += 1;
          break;
        }
        value.push(candidateLine);
        index += 1;
      }
      blocks.push({ kind: "code", language: fence.language, value: value.join("\n") });
      continue;
    }

    const heading = parseHeading(trimmed);
    if (heading !== null) {
      blocks.push({
        kind: "heading",
        level: heading.level,
        nodes: parseInline(heading.value),
      });
      index += 1;
      continue;
    }

    if (isRule(trimmed)) {
      blocks.push({ kind: "rule" });
      index += 1;
      continue;
    }

    const nextLine = lines[index + 1];
    if (nextLine !== undefined && trimmed.includes("|") && isTableSeparator(rustTrim(nextLine))) {
      const headers = splitTableRow(trimmed).map(parseInline);
      index += 2;
      const rows: (readonly (readonly Inline[])[])[] = [];
      while (index < lines.length) {
        const rowLine = lines[index];
        if (rowLine === undefined) {
          break;
        }
        const row = rustTrim(rowLine);
        if (row.length === 0 || !row.includes("|")) {
          break;
        }
        rows.push(splitTableRow(row).map(parseInline));
        index += 1;
      }
      blocks.push({ kind: "table", headers, rows });
      continue;
    }

    if (trimmed.startsWith(">")) {
      const quote: string[] = [];
      while (index < lines.length) {
        const quoteLine = lines[index];
        if (quoteLine === undefined) {
          break;
        }
        const candidate = rustTrimStart(quoteLine);
        if (!candidate.startsWith(">")) {
          break;
        }
        const value = candidate.slice(1);
        quote.push(rustTrim(value.startsWith(" ") ? value.slice(1) : value));
        index += 1;
      }
      blocks.push({ kind: "quote", nodes: parseInline(quote.join("\n")) });
      continue;
    }

    if (unorderedItem(trimmed) !== null) {
      const items: (readonly Inline[])[] = [];
      while (index < lines.length) {
        const itemLine = lines[index];
        if (itemLine === undefined) {
          break;
        }
        const value = unorderedItem(rustTrim(itemLine));
        if (value === null) {
          break;
        }
        items.push(parseInline(value));
        index += 1;
      }
      blocks.push({ kind: "unordered_list", items });
      continue;
    }

    const ordered = orderedItem(trimmed);
    if (ordered !== null) {
      const items: (readonly Inline[])[] = [parseInline(ordered.value)];
      index += 1;
      while (index < lines.length) {
        const itemLine = lines[index];
        if (itemLine === undefined) {
          break;
        }
        const item = orderedItem(rustTrim(itemLine));
        if (item === null) {
          break;
        }
        items.push(parseInline(item.value));
        index += 1;
      }
      blocks.push({ kind: "ordered_list", start: ordered.start, items });
      continue;
    }

    const paragraph: string[] = [rustTrim(line)];
    index += 1;
    while (index < lines.length) {
      const paragraphLine = lines[index];
      if (paragraphLine === undefined) {
        break;
      }
      const candidate = rustTrim(paragraphLine);
      if (candidate.length === 0 || startsBlock(lines, index)) {
        break;
      }
      paragraph.push(candidate);
      index += 1;
    }
    blocks.push({ kind: "paragraph", nodes: parseInline(paragraph.join("\n")) });
  }

  return blocks;
}

function startsBlock(lines: readonly string[], index: number): boolean {
  const line = lines[index];
  if (line === undefined) {
    return false;
  }
  const trimmed = rustTrim(line);
  const nextLine = lines[index + 1];
  return (
    fenceStart(line) !== null ||
    parseHeading(trimmed) !== null ||
    isRule(trimmed) ||
    trimmed.startsWith(">") ||
    unorderedItem(trimmed) !== null ||
    orderedItem(trimmed) !== null ||
    (nextLine !== undefined && trimmed.includes("|") && isTableSeparator(rustTrim(nextLine)))
  );
}

function fenceStart(value: string): FenceStart | null {
  let start = 0;
  while (value.charAt(start) === " ") {
    start += 1;
  }
  if (start > 3) {
    return null;
  }
  const marker = value.charAt(start);
  if (marker !== "`" && marker !== "~") {
    return null;
  }
  let length = 0;
  while (value.charAt(start + length) === marker) {
    length += 1;
  }
  if (length < 3) {
    return null;
  }
  const language = rustTrim(value.slice(start + length));
  return {
    fence: value.slice(start, start + length),
    language: language.length === 0 ? null : language,
  };
}

function isClosingFence(candidate: string, fence: string): boolean {
  if (!candidate.startsWith(fence)) {
    return false;
  }
  const marker = fence.charAt(0);
  let rest = candidate.slice(fence.length);
  while (rest.startsWith(marker)) {
    rest = rest.slice(marker.length);
  }
  return rustTrim(rest).length === 0;
}

function parseHeading(value: string): { readonly level: number; readonly value: string } | null {
  let level = 0;
  while (value.charAt(level) === "#") {
    level += 1;
  }
  if (level < 1 || level > 6) {
    return null;
  }
  const content = value.slice(level);
  if (!content.startsWith(" ") && !content.startsWith("\t")) {
    return null;
  }
  return { level, value: rustTrim(content.slice(1)) };
}

function isRule(value: string): boolean {
  // 沿用只忽略 ASCII 空格的语法，不为每条候选行创建去空格副本。
  let marker: string | null = null;
  let count = 0;
  for (const character of value) {
    if (character === " ") {
      continue;
    }
    if (marker === null) {
      if (character !== "-" && character !== "*" && character !== "_") {
        return false;
      }
      marker = character;
      count = 1;
      continue;
    }
    if (character !== marker) {
      return false;
    }
    count += 1;
  }
  return count >= 3;
}

function isTableSeparator(value: string): boolean {
  // split 即使对空文本也产生一个单元格；空行仍由长度校验拒绝。
  for (const cell of splitTableRow(value)) {
    if (!(utf8ByteLength(cell) >= 3 && [...cell].every(isTableSeparatorCharacter))) {
      return false;
    }
  }
  return true;
}

function isTableSeparatorCharacter(character: string): boolean {
  return character === "-" || character === ":";
}

function splitTableRow(value: string): string[] {
  let row = rustTrim(value);
  if (row.startsWith("|")) {
    row = row.slice(1);
  }
  if (row.endsWith("|")) {
    row = row.slice(0, -1);
  }
  return row.split("|").map(rustTrim);
}

function unorderedItem(value: string): string | null {
  for (const prefix of ["- ", "* ", "+ "]) {
    if (value.startsWith(prefix)) {
      return value.slice(prefix.length);
    }
  }
  return null;
}

function orderedItem(value: string): OrderedItem | null {
  const separator = value.indexOf(". ");
  if (separator < 0) {
    return null;
  }
  const start = parseUsize(value.slice(0, separator));
  return start === null ? null : { start, value: value.slice(separator + 2) };
}

function parseUsize(value: string): number | null {
  let index = value.startsWith("+") ? 1 : 0;
  if (index === value.length) {
    return null;
  }
  let result = 0;
  while (index < value.length) {
    const code = value.charCodeAt(index);
    if (code < 48 || code > 57) {
      return null;
    }
    result = result * 10 + code - 48;
    if (result > MAX_USIZE) {
      return null;
    }
    index += 1;
  }
  return result;
}

function rustLines(value: string): string[] {
  if (value.length === 0) {
    return [];
  }
  const pieces = value.split("\n");
  const endsWithNewline = value.endsWith("\n");
  if (endsWithNewline) {
    pieces.pop();
  }
  return pieces.map((line, index) => {
    const terminated = index < pieces.length - 1 || endsWithNewline;
    return terminated && line.endsWith("\r") ? line.slice(0, -1) : line;
  });
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

function rustTrimStart(value: string): string {
  let start = 0;
  while (start < value.length && isRustWhitespace(value.charCodeAt(start))) {
    start += 1;
  }
  return value.slice(start);
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
