// 登录时长表单的精确单位换算；小数与科学计数法必须得到整分钟，不做浮点舍入。

import { err, ok, type Result, unreachable } from "@auction/web-shared/result";
import { utf8ByteLength } from "@auction/web-shared/text";

import { validSessionDuration } from "../../api/session-preference";

export type DurationUnit = "minutes" | "hours" | "days";

/** 可编辑的登录时长草稿；`permanent` 选中时 amount/unit 仍保留在原处。 */
export interface DurationDraft {
  permanent: boolean;
  amount: string;
  unit: DurationUnit;
}

const U128_MAX = (1n << 128n) - 1n;
const I64_MAX = 9_223_372_036_854_775_807n;

export function durationUnitFromValue(value: string): DurationUnit | null {
  switch (value) {
    case "minutes":
      return "minutes";
    case "hours":
      return "hours";
    case "days":
      return "days";
    default:
      return null;
  }
}

export function durationUnitValue(unit: DurationUnit): string {
  switch (unit) {
    case "minutes":
      return "minutes";
    case "hours":
      return "hours";
    case "days":
      return "days";
    default:
      return unreachable(unit);
  }
}

export function durationUnitMaximum(unit: DurationUnit): string {
  switch (unit) {
    case "minutes":
      return "525600";
    case "hours":
      return "8760";
    case "days":
      return "365";
    default:
      return unreachable(unit);
  }
}

function durationUnitMinutes(unit: DurationUnit): bigint {
  switch (unit) {
    case "minutes":
      return 1n;
    case "hours":
      return 60n;
    case "days":
      return 1_440n;
    default:
      return unreachable(unit);
  }
}

export function durationDraftDefault(): DurationDraft {
  return { permanent: true, amount: "1", unit: "days" };
}

export function durationDraftFromSeconds(seconds: number | null): DurationDraft {
  if (seconds === null) {
    return durationDraftDefault();
  }
  const minutes = Math.trunc(seconds / 60);
  if (minutes % 1_440 === 0) {
    return { permanent: false, amount: (minutes / 1_440).toString(), unit: "days" };
  }
  if (minutes % 60 === 0) {
    return { permanent: false, amount: (minutes / 60).toString(), unit: "hours" };
  }
  return { permanent: false, amount: minutes.toString(), unit: "minutes" };
}

/** 精确解析 i32；Rust 的整数解析不接受空白、下划线和指数。 */
function parseI32(value: string): number | null {
  if (!/^[+-]?[0-9]+$/u.test(value)) {
    return null;
  }
  const parsed = Number(value);
  if (!Number.isSafeInteger(parsed) || parsed < -2_147_483_648 || parsed > 2_147_483_647) {
    return null;
  }
  return parsed;
}

function checkedMultiplyU128(left: bigint, right: bigint): bigint | null {
  const value = left * right;
  return value <= U128_MAX ? value : null;
}

function checkedAddU128(left: bigint, right: bigint): bigint | null {
  const value = left + right;
  return value <= U128_MAX ? value : null;
}

function checkedPow10(exponent: number): bigint | null {
  // u128 只能容纳 10^0..10^38；超过后 Rust checked_pow 返回 None。
  if (exponent < 0 || exponent > 38) {
    return null;
  }
  return 10n ** BigInt(exponent);
}

function exactMinutes(amount: string, unit: DurationUnit): bigint | null {
  if (amount === "" || utf8ByteLength(amount) > 64) {
    return null;
  }
  const exponentMarker = amount.search(/[eE]/u);
  const mantissa = exponentMarker < 0 ? amount : amount.slice(0, exponentMarker);
  const exponent = exponentMarker < 0 ? 0 : parseI32(amount.slice(exponentMarker + 1));
  if (exponent === null) {
    return null;
  }
  let numerator = 0n;
  let fractionDigits = 0;
  let decimal = false;
  let digits = false;
  for (const character of mantissa) {
    if (character === "." && !decimal) {
      decimal = true;
      continue;
    }
    const codePoint = character.codePointAt(0) ?? 0;
    if (codePoint < 0x30 || codePoint > 0x39) {
      return null;
    }
    digits = true;
    const multiplied = checkedMultiplyU128(numerator, 10n);
    if (multiplied === null) {
      return null;
    }
    const added = checkedAddU128(multiplied, BigInt(codePoint - 0x30));
    if (added === null) {
      return null;
    }
    numerator = added;
    if (decimal) {
      fractionDigits += 1;
    }
  }
  if (!digits || numerator === 0n) {
    return null;
  }
  const multipliedByUnit = checkedMultiplyU128(numerator, durationUnitMinutes(unit));
  if (multipliedByUnit === null) {
    return null;
  }
  numerator = multipliedByUnit;

  // fraction_digits - exponent 在 Rust 中是有符号 i32 的 checked_sub；溢出即失败。
  const scale = fractionDigits - exponent;
  if (scale > 2_147_483_647 || scale < -2_147_483_648) {
    return null;
  }
  if (scale > 0) {
    const denominator = checkedPow10(scale);
    if (denominator === null || numerator % denominator !== 0n) {
      return null;
    }
    return numerator / denominator;
  }
  const multiplier = checkedPow10(-scale);
  if (multiplier === null) {
    return null;
  }
  return checkedMultiplyU128(numerator, multiplier);
}

export function durationDraftSeconds(draft: DurationDraft): Result<number | null, null> {
  if (draft.permanent) {
    return ok(null);
  }
  const minutes = exactMinutes(draft.amount, draft.unit);
  if (minutes === null || minutes > I64_MAX / 60n) {
    return err(null);
  }
  const seconds = minutes * 60n;
  if (seconds > I64_MAX) {
    return err(null);
  }
  const numericSeconds = Number(seconds);
  if (!validSessionDuration(numericSeconds)) {
    return err(null);
  }
  return ok(numericSeconds);
}
