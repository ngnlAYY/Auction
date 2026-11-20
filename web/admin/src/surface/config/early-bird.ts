// 赛季早鸟期表单字段。旧 runtime 文档可省略整个区块：未改动时不写入，保持文件与命令账本指纹；
// 服务端对取默认值的赛季前场次字段不做序列化，区块内缺省的字段同样按默认值展示。
// 字段默认值与服务端 `AuctionEarlyBirdConfig::default()` 一致，完整规则仍由服务端校验。

import type { AdminJsonValue } from "@auction/contracts/dto/admin/rules";
import { err, ok, type Result } from "@auction/web-shared/result";

import { trimRustWhitespace } from "../../api/session-time";
import { MAX_JSON_SAFE_INTEGER } from "../../api/validation";

/** 早鸟区块在 runtime 文档中的路径。 */
export const ROOT = "operations.schedule.early_bird";

export const TEXT_FIELDS: readonly string[] = [
  "operations.schedule.early_bird.timezone",
  "operations.schedule.early_bird.season_start_time",
  "operations.schedule.early_bird.open_time",
];

export const LIST_FIELDS: readonly string[] = [
  "operations.schedule.early_bird.worlds",
  "operations.schedule.early_bird.season_starts",
  "operations.schedule.early_bird.qualities",
];

/** 按行输入的正整数列表。 */
export const NUMBER_LIST_FIELDS: readonly string[] = [
  "operations.schedule.early_bird.session_days",
];

export const NUMBER_FIELDS: readonly string[] = [
  "operations.schedule.early_bird.window_days",
  "operations.schedule.early_bird.preview_lead_minutes",
  "operations.schedule.early_bird.live_minutes",
  "operations.schedule.early_bird.pre_season_days_before",
];

/** 与服务端默认值一致的完整区块；每次调用都返回独立可变对象。 */
export function defaultBlock(): Record<string, unknown> {
  return {
    enabled: false,
    timezone: "UTC",
    worlds: ["isekai"],
    season_starts: ["05-01", "11-01"],
    season_start_time: "09:00",
    window_days: 7,
    session_days: [1, 3, 5, 7],
    open_time: "18:00",
    preview_lead_minutes: 120,
    live_minutes: 1440,
    pre_season_session: true,
    pre_season_days_before: 3,
    qualities: [],
    pause_intake_before_season: true,
  };
}

/** 读取文档中的早鸟区块；缺省时返回 `null`。 */
export function block(document: unknown): unknown | null {
  let current: unknown = document;
  for (const key of ["operations", "schedule", "early_bird"]) {
    if (!isJsonObject(current) || !Object.hasOwn(current, key)) {
      return null;
    }
    current = current[key];
  }
  return current;
}

/** 区块字段名，例如 `operations.schedule.early_bird.timezone` 返回 `timezone`。 */
export function field(path: string): string | null {
  if (!path.startsWith(ROOT)) {
    return null;
  }
  const rest = path.slice(ROOT.length);
  return rest.startsWith(".") ? rest.slice(1) : null;
}

/** 把区块（缺省时为默认区块）投影为输入框文本；列表按行展示，区块内缺省的字段取默认值。 */
export function inputs(document: unknown): Result<readonly (readonly [string, string])[], null> {
  const defaults = defaultBlock();
  const source = block(document);
  const value = (path: string): unknown => {
    const name = field(path);
    if (name === null) {
      return null;
    }
    if (isJsonObject(source) && Object.hasOwn(source, name)) {
      return source[name];
    }
    return defaults[name];
  };
  const result: Array<readonly [string, string]> = [];
  for (const path of TEXT_FIELDS) {
    const item = value(path);
    if (typeof item !== "string") {
      return err(null);
    }
    result.push([path, item]);
  }
  for (const path of LIST_FIELDS) {
    const items = value(path);
    if (!Array.isArray(items) || !items.every((item) => typeof item === "string")) {
      return err(null);
    }
    result.push([path, items.join("\n")]);
  }
  for (const path of NUMBER_LIST_FIELDS) {
    const items = value(path);
    if (!Array.isArray(items)) {
      return err(null);
    }
    const numbers: string[] = [];
    for (const item of items) {
      const number = safeU64(item);
      if (number === null) {
        return err(null);
      }
      numbers.push(number.toString());
    }
    result.push([path, numbers.join("\n")]);
  }
  for (const path of NUMBER_FIELDS) {
    const number = safeU64(value(path));
    if (number === null) {
      return err(null);
    }
    result.push([path, number.toString()]);
  }
  return ok(result);
}

/** 开关字段在区块或字段缺省时读取默认值，避免把默认开启的暂停收件、赛季前场次显示为关闭。 */
export function boolean(document: unknown, path: string): boolean | null {
  const name = field(path);
  if (name === null) {
    return null;
  }
  const source = block(document);
  if (isJsonObject(source) && Object.hasOwn(source, name)) {
    const value = source[name];
    if (typeof value === "boolean") {
      return value;
    }
  }
  const fallback = defaultBlock()[name];
  return typeof fallback === "boolean" ? fallback : null;
}

/** 确保文档存在早鸟区块；缺省时写入默认区块后再由调用方覆盖具体字段。 */
export function ensureBlock(document: unknown): Result<Record<string, unknown>, null> {
  const operations = ownObject(document, "operations");
  if (operations === null) {
    return err(null);
  }
  const schedule = ownObject(operations, "schedule");
  if (schedule === null) {
    return err(null);
  }
  if (!Object.hasOwn(schedule, "early_bird")) {
    schedule.early_bird = defaultBlock();
  }
  const earlyBird = schedule.early_bird;
  if (!isJsonObject(earlyBird)) {
    return err(null);
  }
  return ok(earlyBird);
}

/** 输入框的提交值；数字或数字列表处于非法中间态时拒绝提交。 */
export function submittedValue(path: string, input: string): Result<AdminJsonValue, null> {
  const lines = (): string[] =>
    rustLines(input)
      .map(trimRustWhitespace)
      .filter((line) => line !== "");
  if (TEXT_FIELDS.includes(path)) {
    return ok(input);
  }
  if (LIST_FIELDS.includes(path)) {
    return ok(lines());
  }
  if (NUMBER_LIST_FIELDS.includes(path)) {
    const numbers: AdminJsonValue[] = [];
    for (const line of lines()) {
      const number = parseRustU64(line);
      if (number === null) {
        return err(null);
      }
      numbers.push(number);
    }
    return ok(numbers);
  }
  if (NUMBER_FIELDS.includes(path)) {
    const number = parseRustU64(input);
    return number === null ? err(null) : ok(number);
  }
  return err(null);
}

/** 所有可编辑输入路径。 */
export function inputPaths(): readonly string[] {
  return [...TEXT_FIELDS, ...LIST_FIELDS, ...NUMBER_LIST_FIELDS, ...NUMBER_FIELDS];
}

function isJsonObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function ownObject(value: unknown, key: string): Record<string, unknown> | null {
  if (!isJsonObject(value) || !Object.hasOwn(value, key)) {
    return null;
  }
  const child = value[key];
  return isJsonObject(child) ? child : null;
}

/** 与 serde_json 的 `as_u64` 在当前浏览器精度内一致的读取：只接受 JSON 安全整数。 */
function safeU64(value: unknown): number | null {
  return typeof value === "number" &&
    Number.isSafeInteger(value) &&
    value >= 0 &&
    value <= MAX_JSON_SAFE_INTEGER
    ? value
    : null;
}

/** Rust `u64::from_str` 的十进制语法；超过 JS 安全整数时按既有精度边界拒绝。 */
function parseRustU64(value: string): number | null {
  if (!/^\+?[0-9]+$/u.test(value)) {
    return null;
  }
  const number = Number(value);
  return Number.isSafeInteger(number) && number <= MAX_JSON_SAFE_INTEGER ? number : null;
}

/** Rust `str::lines`：按 `\n` 分行，`\r\n` 去掉 `\r`，末尾换行不产生空行。 */
function rustLines(value: string): string[] {
  const lines: string[] = [];
  let start = 0;
  for (let index = 0; index < value.length; index += 1) {
    if (value.charAt(index) !== "\n") {
      continue;
    }
    let line = value.slice(start, index);
    if (line.endsWith("\r")) {
      line = line.slice(0, -1);
    }
    lines.push(line);
    start = index + 1;
  }
  if (start < value.length) {
    lines.push(value.slice(start));
  }
  return lines;
}
