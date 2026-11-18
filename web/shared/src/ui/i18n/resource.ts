// FTL 资源的平铺子集校验、bundle 构造与按需取回。
// 子集之外的写法整份拒绝，避免把解析器吞掉的语法错误当作语言切换成功。

import { FluentBundle, FluentResource } from "@fluent/bundle";

import { sleep } from "../../platform/timer";
import type { TaskContext } from "../../reactive/tasks";
import { err, ok, type Result } from "../../result";
import type { LocaleCatalog, LocaleCode, LocaleSource } from "./catalog";

const MESSAGE_LINE = /^([A-Za-z][A-Za-z0-9_-]*)( *)=( *)(.+)$/;

/**
 * 由默认 FTL 与可选目标语言 FTL 构造 bundle；目标缺键时回退默认语言。
 *
 * 加载契约：只接受仓库实际使用的平铺 Fluent 子集。空行与以 `#` 开头的注释行合法；
 * 消息必须是单行 `标识符 = 文本`，文本中的占位只有 `{ $变量 }` 与 `{ "字符串" }` 两种。
 * 续行、属性、术语、选择表达式、消息引用、函数调用以及子集之外的裸花括号都拒绝整份资源。
 */
export function buildBundle(
  defaultLocale: LocaleCode,
  defaultFtl: string,
  targetLocale: LocaleCode,
  targetFtl: string | null,
): Result<FluentBundle, string> {
  try {
    const defaultResource = parseFluentResource(defaultFtl);
    if (!defaultResource.ok) {
      return err(defaultResource.error);
    }
    const bundle = new FluentBundle([targetLocale], { useIsolating: true });
    const defaultErrors = bundle.addResource(defaultResource.value);
    if (defaultErrors.length > 0) {
      return err(fluentErrorsText(defaultErrors));
    }
    if (targetLocale !== defaultLocale && targetFtl !== null) {
      const targetResource = parseFluentResource(targetFtl);
      if (!targetResource.ok) {
        return err(targetResource.error);
      }
      // 源码使用 add_resource_overriding：目标语言覆盖默认文案，资源自身的覆盖错误被忽略。
      bundle.addResource(targetResource.value, { allowOverrides: true });
    }
    return ok(bundle);
  } catch (failure) {
    return err(errorText(failure));
  }
}

/** 取回非默认语言的同源 FTL；语言码没有对应资源时按失败处理。 */
export async function fetchCatalogLocale(
  catalog: LocaleCatalog,
  target: LocaleCode,
  context: TaskContext,
): Promise<Result<string | null, string>> {
  const source = localeSource(catalog, target);
  if (source === null) {
    return err("unsupported locale");
  }
  const fetched = await fetchLocaleText(source.url, context.signal);
  if (!context.alive()) {
    return err("locale fetch cancelled");
  }
  return fetched.ok ? ok<string | null>(fetched.value) : fetched;
}

/** 供会话错误信息复用稳定文本；异常对象仍只暴露其消息。 */
export function errorText(failure: unknown): string {
  try {
    return failure instanceof Error ? failure.message : String(failure);
  } catch {
    return "未知错误";
  }
}

function localeSource(catalog: LocaleCatalog, target: LocaleCode): LocaleSource | null {
  return catalog.sources.find((candidate) => candidate.id === target) ?? null;
}

/** 超时或非 2xx 都按失败处理，由调用方保留当前语言。 */
async function fetchLocaleText(url: string, signal: AbortSignal): Promise<Result<string, string>> {
  if (signal.aborted) {
    return err("locale fetch cancelled");
  }
  const request = new AbortController();
  const abortRequest = () => request.abort();
  signal.addEventListener("abort", abortRequest, { once: true });
  try {
    return await Promise.race([
      fetchLocaleTextDirect(url, request.signal),
      sleep(3000, request.signal).then((elapsed) =>
        err(elapsed ? "locale fetch timeout" : "locale fetch cancelled"),
      ),
    ]);
  } finally {
    signal.removeEventListener("abort", abortRequest);
    // 等待结束时同时收口计时器和 fetch，超时不能留下在途正文读取。
    abortRequest();
  }
}

async function fetchLocaleTextDirect(
  url: string,
  signal: AbortSignal,
): Promise<Result<string, string>> {
  let response: Response;
  try {
    response = await fetch(url, { method: "GET", signal });
  } catch {
    return err("locale fetch failed");
  }
  if (!response.ok) {
    return err("locale fetch status");
  }
  try {
    return ok(await response.text());
  } catch {
    return err("locale read failed");
  }
}

function parseFluentResource(text: string): Result<FluentResource, string> {
  const messages = fluentSubsetMessages(text);
  if (messages === null) {
    return err("invalid fluent resource");
  }
  try {
    // 只把通过校验的消息行交给解析器：注释里的内容（包括解析器可能当作换行的字符）不参与解析。
    return ok(new FluentResource(messages.join("\n")));
  } catch (failure) {
    return err(errorText(failure));
  }
}

/** 按平铺子集逐行校验；全部合法时返回消息行（不含空行与注释），否则返回 null。 */
function fluentSubsetMessages(text: string): readonly string[] | null {
  const messages: string[] = [];
  for (const line of text.split(/\r\n|\n|\r/)) {
    if (line.length === 0 || line.startsWith("#")) {
      continue;
    }
    const message = MESSAGE_LINE.exec(line);
    const body = message?.[4];
    if (body === undefined || body.trim().length === 0 || !fluentBodyFitsSubset(body)) {
      return null;
    }
    messages.push(line);
  }
  return messages;
}

function fluentBodyFitsSubset(text: string): boolean {
  for (let index = 0; index < text.length; ) {
    const character = text.charAt(index);
    if (character === "}") {
      return false;
    }
    if (character !== "{") {
      index += 1;
      continue;
    }
    const end = fluentPlaceholderEnd(text, index);
    if (end === null) {
      return false;
    }
    index = end;
  }
  return true;
}

function fluentPlaceholderEnd(text: string, start: number): number | null {
  let cursor = skipPlaceholderWhitespace(text, start + 1);
  const first = text.charAt(cursor);
  if (first === "$") {
    cursor += 1;
    if (!isFluentIdentifierStart(text.charAt(cursor))) {
      return null;
    }
    cursor += 1;
    while (isFluentIdentifierContinue(text.charAt(cursor))) {
      cursor += 1;
    }
  } else if (first === '"') {
    const stringEnd = fluentStringEnd(text, cursor);
    if (stringEnd === null) {
      return null;
    }
    cursor = stringEnd;
  } else {
    return null;
  }
  cursor = skipPlaceholderWhitespace(text, cursor);
  return text.charAt(cursor) === "}" ? cursor + 1 : null;
}

function fluentStringEnd(text: string, openQuote: number): number | null {
  let cursor = openQuote + 1;
  while (cursor < text.length) {
    const character = text.charAt(cursor);
    if (character === '"') {
      return cursor + 1;
    }
    if (character !== "\\") {
      cursor += 1;
      continue;
    }
    const escapeEnd = fluentEscapeEnd(text, cursor);
    if (escapeEnd === null) {
      return null;
    }
    cursor = escapeEnd;
  }
  return null;
}

function fluentEscapeEnd(text: string, backslash: number): number | null {
  const escaped = text.charAt(backslash + 1);
  if (escaped === "\\" || escaped === '"') {
    return backslash + 2;
  }
  if (escaped === "u" && isHexSlice(text, backslash + 2, 4)) {
    return backslash + 6;
  }
  if (escaped === "U" && isHexSlice(text, backslash + 2, 6)) {
    return backslash + 8;
  }
  return null;
}

function isHexSlice(text: string, start: number, length: number): boolean {
  if (start + length > text.length) {
    return false;
  }
  for (let index = start; index < start + length; index += 1) {
    const code = text.charCodeAt(index);
    const hexadecimal =
      (code >= 48 && code <= 57) || (code >= 65 && code <= 70) || (code >= 97 && code <= 102);
    if (!hexadecimal) {
      return false;
    }
  }
  return true;
}

function skipPlaceholderWhitespace(text: string, start: number): number {
  let cursor = start;
  while (text.charAt(cursor) === " " || text.charAt(cursor) === "\t") {
    cursor += 1;
  }
  return cursor;
}

function isFluentIdentifierStart(character: string): boolean {
  const code = character.charCodeAt(0);
  return (code >= 65 && code <= 90) || (code >= 97 && code <= 122);
}

function isFluentIdentifierContinue(character: string): boolean {
  const code = character.charCodeAt(0);
  return (
    isFluentIdentifierStart(character) || (code >= 48 && code <= 57) || code === 95 || code === 45
  );
}

function fluentErrorsText(errors: readonly Error[]): string {
  return errors.map((failure) => errorText(failure)).join("; ");
}
