// 装备属性的展示排序与数值格式：游戏投影中的基础值和 roll 百分比按 Rust 的默认
// 浮点显示与 `{:.1}` 规则格式化，避免浏览器默认格式化把极小或极大值改成科学计数法。

import type { PublicEquipmentStatResponse } from "@auction/contracts/dto/public/catalog/lot";
import { compareText } from "@auction/web-shared/text";

interface StatRow {
  readonly name: string;
  readonly value: string;
}
export function statRows(stats: Record<string, PublicEquipmentStatResponse>): StatRow[] {
  return Object.entries(stats)
    .sort(([left], [right]) => compareText(left, right))
    .map(([name, stat]) => ({
      name,
      value: statValueLabel(stat.value, stat.base, stat.roll),
    }));
}
/** 属性值后附带括号里的 roll：游戏给出的 0–200 基础值与对应的百分比，如 `+109.1(195/97.5%)`。
 * 没有 roll 的属性（元素伤害、元素减伤等）只显示数值；只有其中一项时括号里只放那一项。
 * 两个数都来自服务端投影，这里不互相换算；百分比保留一位小数，整数不带小数位。 */
function statValueLabel(
  value: string,
  base: number | null | undefined,
  roll: number | null | undefined,
): string {
  const rollLabel = roll == null ? null : `${formatShortestFloat(roundTenthsAwayFromZero(roll))}%`;
  if (base != null && rollLabel !== null) {
    return `${value}(${formatShortestFloat(base)}/${rollLabel})`;
  }
  if (base != null) {
    return `${value}(${formatShortestFloat(base)})`;
  }
  if (rollLabel !== null) {
    return `${value}(${rollLabel})`;
  }
  return value;
}

/** Rust `f64::round` 的语义：整数部分取绝对值后四舍五入，恰好半时远离零。 */
function roundTenthsAwayFromZero(value: number): number {
  const scaled = value * 10;
  if (!Number.isFinite(scaled)) {
    return scaled / 10;
  }
  const negative = scaled < 0 || Object.is(scaled, -0);
  const rounded = roundToIntegerAwayFromZero(Math.abs(scaled));
  return (negative ? -Number(rounded) : Number(rounded)) / 10;
}

/** Rust 默认浮点 Display 的有限值形式：最短十进制、整数不带 `.0`，且不使用科学计数法。 */
function formatShortestFloat(value: number): string {
  if (Number.isNaN(value)) {
    return "NaN";
  }
  if (value === Infinity) {
    return "inf";
  }
  if (value === -Infinity) {
    return "-inf";
  }
  if (Object.is(value, -0)) {
    return "-0";
  }
  const negative = value < 0;
  const magnitude = Math.abs(value);
  if (magnitude === 0) {
    return "0";
  }
  // `String` 的最短十进制只用来确定有效数字位数与首位量级；同量级的两个十进制表示可能
  // 都能回读到同一个 f64，JS 与 Rust 对并列尾数的选择不同（例如 Rust 选 667082108456853.3，
  // JS 选 667082108456853.2）。因此再用 f64 的精确二进制值按 Rust 的规则重新做一次十进制舍入。
  const shape = shortestFloatShape(magnitude);
  const { significand, exponent } = finiteBinaryParts(magnitude);
  const shift = shape.digits.length - 1 - shape.exponent;
  let numerator = significand;
  let denominator = 1n;
  if (shift >= 0) {
    numerator *= 10n ** BigInt(shift);
  } else {
    denominator *= 10n ** BigInt(-shift);
  }
  if (exponent >= 0) {
    numerator <<= BigInt(exponent);
  } else {
    denominator <<= BigInt(-exponent);
  }
  let coefficient = numerator / denominator;
  const remainder = numerator % denominator;
  if (remainder * 2n >= denominator) {
    coefficient += 1n;
  }
  let coefficientText = coefficient.toString();
  let decimalExponent = shape.exponent;
  if (coefficientText.length > shape.digits.length) {
    // 999…9 舍入成 1000…0 时少保留一个零，同时把量级进一位。
    coefficientText = coefficientText.slice(0, -1);
    decimalExponent += 1;
  }
  const decimalPosition = decimalExponent + 1;
  const signedDigits = `${negative ? "-" : ""}${coefficientText}`;
  const unsignedDigits = coefficientText;
  if (decimalPosition <= 0) {
    return `${negative ? "-" : ""}0.${"0".repeat(-decimalPosition)}${unsignedDigits}`;
  }
  if (decimalPosition >= coefficientText.length) {
    return `${signedDigits}${"0".repeat(decimalPosition - coefficientText.length)}`;
  }
  return `${negative ? "-" : ""}${coefficientText.slice(0, decimalPosition)}.${coefficientText.slice(decimalPosition)}`;
}

interface FloatShape {
  readonly digits: string;
  readonly exponent: number;
}

/** 从 JS 的最短十进制写法取出有效数字与首位指数（`digits` 不保留首尾零）。 */
function shortestFloatShape(value: number): FloatShape {
  const text = String(value).toLowerCase();
  const [mantissa = "", exponentText] = text.split("e");
  const [whole = "", fraction = ""] = mantissa.split(".");
  const allDigits = `${whole}${fraction}`;
  const first = allDigits.search(/[1-9]/);
  const digits = allDigits.slice(first).replace(/0+$/, "");
  return {
    digits,
    exponent: (exponentText === undefined ? 0 : Number(exponentText)) + whole.length - first - 1,
  };
}

interface BinaryFloatParts {
  readonly significand: bigint;
  readonly exponent: number;
}

/** 取得有限非零 f64 的精确二进制有效数与指数。 */
function finiteBinaryParts(value: number): BinaryFloatParts {
  const view = new DataView(new ArrayBuffer(8));
  view.setFloat64(0, value, false);
  const bits = view.getBigUint64(0, false);
  const exponentBits = Number((bits >> 52n) & 0x7ffn);
  const fraction = bits & 0xfffffffffffffn;
  return {
    significand: exponentBits === 0 ? fraction : (1n << 52n) | fraction,
    exponent: exponentBits === 0 ? -1074 : exponentBits - 1023 - 52,
  };
}

/** 把有限正 f64 舍入到整数；恰好半时向远离零的方向进位。 */
function roundToIntegerAwayFromZero(value: number): bigint {
  if (value === 0) {
    return 0n;
  }
  const { significand, exponent } = finiteBinaryParts(value);
  let numerator = significand;
  let denominator = 1n;
  if (exponent >= 0) {
    numerator <<= BigInt(exponent);
  } else {
    denominator <<= BigInt(-exponent);
  }
  let quotient = numerator / denominator;
  const remainder = numerator % denominator;
  if (remainder * 2n >= denominator) {
    quotient += 1n;
  }
  return quotient;
}

/** Rust `{:.1}` 的有限值显示：总是保留一位小数，恰好半时取偶（与格式化器的舍入一致）。 */
export function formatOneDecimal(value: number): string {
  if (Number.isNaN(value)) {
    return "NaN";
  }
  if (value === Infinity) {
    return "inf";
  }
  if (value === -Infinity) {
    return "-inf";
  }
  const negative = value < 0 || Object.is(value, -0);
  const magnitude = Math.abs(value);
  // 按 f64 的精确二进制值做十进制舍入；先乘 10 再取整会把 0.05 这类二进制略大于十进制
  // 半值的数误判成精确半值，`toFixed` 也无法覆盖全部量级。
  const { significand, exponent } = finiteBinaryParts(magnitude);
  let numerator = significand * 10n;
  let denominator = 1n;
  if (exponent >= 0) {
    numerator <<= BigInt(exponent);
  } else {
    denominator <<= BigInt(-exponent);
  }
  let rounded = numerator / denominator;
  const remainder = numerator % denominator;
  const doubledRemainder = remainder * 2n;
  if (doubledRemainder > denominator || (doubledRemainder === denominator && rounded % 2n === 1n)) {
    rounded += 1n;
  }
  const digits = rounded.toString().padStart(2, "0");
  return `${negative ? "-" : ""}${digits.slice(0, -1)}.${digits.slice(-1)}`;
}
