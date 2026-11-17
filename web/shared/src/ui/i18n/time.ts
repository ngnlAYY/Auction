// 按显式语言把 RFC3339 时间显示为 Asia/Shanghai 时间。
// 浏览器格式化失败时使用与源码相同的 UTC+8 兜底，并遵守 auction_time::Timestamp 的范围。

const RUST_WHITESPACE_CLASS =
  "[\\u0009-\\u000D\\u0020\\u0085\\u00A0\\u1680\\u2000-\\u200A\\u2028\\u2029\\u202F\\u205F\\u3000]";
const RELAXED_TIMESTAMP = new RegExp(
  `^${RUST_WHITESPACE_CLASS}*((?:[+-]\\d+)|(?:\\d{1,4}))${RUST_WHITESPACE_CLASS}*-` +
    `${RUST_WHITESPACE_CLASS}*(\\d{1,2})${RUST_WHITESPACE_CLASS}*-${RUST_WHITESPACE_CLASS}*(\\d{1,2})` +
    `[Tt ]${RUST_WHITESPACE_CLASS}*(\\d{1,2})${RUST_WHITESPACE_CLASS}*:${RUST_WHITESPACE_CLASS}*(\\d{1,2})` +
    `${RUST_WHITESPACE_CLASS}*:${RUST_WHITESPACE_CLASS}*(\\d{1,2})(?:\\.\\d+)?${RUST_WHITESPACE_CLASS}*` +
    `(?:(?:Z|z)|(?:[Uu][Tt][Cc])|([+\\-\\u2212])(\\d{2})(?::|${RUST_WHITESPACE_CLASS})*(\\d{2}))` +
    `${RUST_WHITESPACE_CLASS}*$`,
);

// auction_time::Timestamp 的 UTC 边界：-9999-01-02T01:59:59Z 与
// 9999-12-30T22:00:00.999999999Z。JS Date 的年份范围更宽，不能代替这组边界。
const AUCTION_TIMESTAMP_MIN_MILLIS = -377_705_023_201_000;
const AUCTION_TIMESTAMP_MAX_MILLIS = 253_402_207_200_000;
const SHANGHAI_OFFSET_MILLIS = 8 * 3600 * 1000;

let cachedTimeFormatter: { language: string; formatter: Intl.DateTimeFormat } | null = null;

/** 按显式语言把 RFC3339 时间显示为 Asia/Shanghai 时间，不读取 DOM。 */
export function formatLocalizedTimeForLanguage(value: string, language: string): string {
  if (value.length === 0) {
    return "—";
  }
  const parsed = Date.parse(value);
  if (Number.isFinite(parsed)) {
    try {
      return timeFormatter(language).format(new Date(parsed));
    } catch {
      // 浏览器格式化失败时继续走与源码相同的 UTC+8 兜底格式。
    }
  }
  return shanghaiFallbackText(value) ?? "—";
}

function timeFormatter(language: string): Intl.DateTimeFormat {
  if (cachedTimeFormatter?.language === language) {
    return cachedTimeFormatter.formatter;
  }
  // 同一渲染树只使用一种语言；单槽缓存避免逐时间单元格构造 Intl，也不随语言输入无限增长。
  const formatter = new Intl.DateTimeFormat([language], {
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
    month: "short",
    second: "2-digit",
    year: "numeric",
    hour12: false,
    timeZone: "Asia/Shanghai",
  });
  cachedTimeFormatter = { language, formatter };
  return formatter;
}

/** 兜底格式 `DD Mon YYYY, HH:MM:SS`：平移八小时后直接取 UTC 字段。 */
function shanghaiFallbackText(value: string): string | null {
  const match = RELAXED_TIMESTAMP.exec(value);
  if (match === null) return null;
  const [, yearText, monthText, dayText, hourText, minuteText, secondText] = match;
  if (!yearText || !monthText || !dayText || !hourText || !minuteText || !secondText) return null;
  const year = Number(yearText);
  const month = Number(monthText);
  const day = Number(dayText);
  const hour = Number(hourText);
  const minute = Number(minuteText);
  const second = Number(secondText);
  if (!Number.isInteger(year) || year < -9_999 || year > 9_999) return null;
  if (month < 1 || month > 12 || day < 1 || day > 31) return null;
  if (hour < 0 || hour > 23 || minute < 0 || minute > 59 || second < 0 || second > 59) return null;
  let offsetSeconds = 0;
  const offsetSign = match[7];
  const offsetHours = match[8];
  const offsetMinutes = match[9];
  if (offsetSign !== undefined && offsetHours !== undefined && offsetMinutes !== undefined) {
    const offsetMinuteValue = Number(offsetMinutes);
    offsetSeconds = Number(offsetHours) * 3600 + offsetMinuteValue * 60;
    if (offsetMinuteValue > 59 || offsetSeconds >= 86_400) return null;
    if (offsetSign === "-" || offsetSign === "−") {
      offsetSeconds = -offsetSeconds;
    }
  }
  const utcMillis = civilUtcMillis(year, month, day, hour, minute, second);
  if (utcMillis === null) return null;
  const instantMillis = utcMillis - offsetSeconds * 1000;
  if (!Number.isSafeInteger(instantMillis)) return null;
  if (instantMillis < AUCTION_TIMESTAMP_MIN_MILLIS) return null;
  if (instantMillis > AUCTION_TIMESTAMP_MAX_MILLIS) return null;
  const localMillis = instantMillis + SHANGHAI_OFFSET_MILLIS;
  if (!Number.isSafeInteger(localMillis) || localMillis > AUCTION_TIMESTAMP_MAX_MILLIS) return null;
  const local = new Date(localMillis);
  const localYear = local.getUTCFullYear();
  if (localYear < -9_999 || localYear > 9_999) return null;
  const months = "Jan Feb Mar Apr May Jun Jul Aug Sep Oct Nov Dec".split(" ");
  const monthName = months[local.getUTCMonth()];
  if (monthName === undefined) return null;
  const localDay = String(local.getUTCDate()).padStart(2, "0");
  const localYearText =
    localYear < 0 ? `-${String(-localYear).padStart(3, "0")}` : String(localYear).padStart(4, "0");
  const localHour = String(local.getUTCHours()).padStart(2, "0");
  const localMinute = String(local.getUTCMinutes()).padStart(2, "0");
  const localSecond = String(local.getUTCSeconds()).padStart(2, "0");
  return `${localDay} ${monthName} ${localYearText}, ${localHour}:${localMinute}:${localSecond}`;
}

function civilUtcMillis(
  year: number,
  month: number,
  day: number,
  hour: number,
  minute: number,
  second: number,
): number | null {
  const date = new Date(0);
  date.setUTCFullYear(year, month - 1, day);
  date.setUTCHours(hour, minute, second, 0);
  if (date.getUTCFullYear() !== year || date.getUTCMonth() !== month - 1) return null;
  if (date.getUTCDate() !== day || date.getUTCHours() !== hour) return null;
  if (date.getUTCMinutes() !== minute || date.getUTCSeconds() !== second) return null;
  const milliseconds = date.getTime();
  return Number.isFinite(milliseconds) ? milliseconds : null;
}
