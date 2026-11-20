// Admin 本地日期时间的 UTC 往返、缺口与回拨消歧回归。

import { describe, expect, test } from "bun:test";

import { datetimeLocalValue, parseLocalDatetime } from "./validation";

function withTimeZone(timeZone: string, run: () => void): void {
  // Bun 删除原先缺省的 TZ 后可能留下已切换的运行时时区；偏移从 IANA 时区数据独立取得。
  const clock = new Intl.DateTimeFormat("en", { timeZone, timeZoneName: "longOffset" });
  withLocalOffset((timestamp) => {
    const offset = clock
      .formatToParts(timestamp)
      .find(({ type }) => type === "timeZoneName")?.value;
    if (offset === "GMT") return 0;
    const match = /^GMT([+-])(\d{2}):(\d{2})(?::(\d{2}))?$/.exec(offset ?? "");
    if (match === null) throw new Error(`时区偏移无法读取：${timeZone}`);
    const [, sign, hours, minutes, seconds = "0"] = match;
    const offsetSeconds = Number(hours) * 3_600 + Number(minutes) * 60 + Number(seconds);
    return (sign === "-" ? -offsetSeconds : offsetSeconds) * 1_000;
  }, run);
}

/** 只替换 Date 的本地字段读取，UTC 字段与纪元毫秒保持真实值。 */
function withLocalOffset(offsetAt: (timestamp: number) => number, run: () => void): void {
  const RealDate = Date;
  class LocalDate extends RealDate {
    constructor(value?: number | string | Date) {
      super(
        value === undefined ? RealDate.now() : value instanceof RealDate ? value.getTime() : value,
      );
    }

    private local(): Date {
      const timestamp = this.getTime();
      return new RealDate(timestamp + offsetAt(timestamp));
    }

    override getFullYear(): number {
      return this.local().getUTCFullYear();
    }

    override getMonth(): number {
      return this.local().getUTCMonth();
    }

    override getDate(): number {
      return this.local().getUTCDate();
    }

    override getHours(): number {
      return this.local().getUTCHours();
    }

    override getMinutes(): number {
      return this.local().getUTCMinutes();
    }

    override getSeconds(): number {
      return this.local().getUTCSeconds();
    }
  }
  Object.defineProperty(globalThis, "Date", { value: LocalDate });
  try {
    run();
  } finally {
    Object.defineProperty(globalThis, "Date", { value: RealDate });
  }
}

describe("Admin 本地日期时间校验（对应 Rust api/validation.rs）", () => {
  test("local_minute_round_trips_through_utc", () => {
    // Rust 宿主构建的“本地时区”退化为 UTC；这里固定 UTC 复现同一组字面量。
    withTimeZone("UTC", () => {
      expect(datetimeLocalValue("2026-11-14T09:05:00Z")).toBe("2026-11-14T09:05");
      const instant = parseLocalDatetime("2026-11-14T09:05");
      expect(instant).toEqual({ ok: true, value: "2026-11-14T09:05:00Z" });
      if (!instant.ok) throw new Error("本地时间未解析成功");
      expect(datetimeLocalValue(instant.value)).toBe("2026-11-14T09:05");
    });
  });

  test("local_value_drops_seconds_and_rejects_unparseable_text", () => {
    withTimeZone("UTC", () => {
      expect(datetimeLocalValue("2026-11-14T09:05:37.250+08:00")).toBe("2026-11-14T01:05");
      expect(datetimeLocalValue("")).toBeNull();
      expect(datetimeLocalValue("not a time")).toBeNull();
      expect(parseLocalDatetime("")).toEqual({ ok: false, error: "invalid" });
    });
  });

  test("spring_forward_accepts_a_valid_time_and_rejects_a_skipped_one", () => {
    withTimeZone("America/Los_Angeles", () => {
      expect(parseLocalDatetime("2027-03-14T03:30")).toEqual({
        ok: true,
        value: "2027-03-14T10:30:00Z",
      });
      expect(parseLocalDatetime("2027-03-14T03:30:19.123456789")).toEqual({
        ok: true,
        value: "2027-03-14T10:30:19.123456789Z",
      });
      expect(parseLocalDatetime("2027-03-14T02:30")).toEqual({
        ok: false,
        error: "nonexistent",
      });
      expect(parseLocalDatetime("2027-02-29T12:00")).toEqual({
        ok: false,
        error: "invalid",
      });
    });
  });

  test("rejects_repeated_times_without_assuming_a_one_hour_shift", () => {
    withTimeZone("America/Los_Angeles", () => {
      expect(parseLocalDatetime("2027-11-07T01:45")).toEqual({
        ok: false,
        error: "ambiguous",
      });
      expect(parseLocalDatetime("2027-11-07T02:00")).toEqual({
        ok: true,
        value: "2027-11-07T10:00:00Z",
      });
    });
    // Rust 另外用合成的 30 分钟回拨验证“不能假定固定一小时”；内存 Date 替身经同一公开入口复现。
    const transition = Date.parse("2027-11-07T09:00:00Z");
    for (const shift of [30, 60] as const) {
      withLocalOffset(
        (timestamp) => (timestamp < transition ? -420 : -420 - shift) * 60_000,
        () => {
          expect(parseLocalDatetime("2027-11-07T01:45")).toEqual({
            ok: false,
            error: "ambiguous",
          });
          expect(parseLocalDatetime("2027-11-07T02:00")).toEqual({
            ok: true,
            value: shift === 30 ? "2027-11-07T09:30:00Z" : "2027-11-07T10:00:00Z",
          });
        },
      );
    }
  });
});
