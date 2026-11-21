// Public 与 Admin 共用的金额、持有时间与 ppm 文本格式化。

/** 金额简称的分母：百万、十亿。 */
const MILLION = 1_000_000;
const BILLION = 1_000_000_000;

/** 表格中的金额：百万级以上且能整千整除时用 M/B，否则回退到千分位全量。 */
export function formatCreditAmount(value: number): string {
  const magnitude = Math.abs(value);
  if (magnitude >= BILLION && magnitude % 1_000_000 === 0) {
    return formatScaledAmount(value, BILLION, "B");
  }
  if (magnitude >= MILLION && magnitude % 1000 === 0) {
    return formatScaledAmount(value, MILLION, "M");
  }
  if (magnitude % 1000 === 0) {
    const sign = value < 0 ? "-" : "";
    return `${sign}${Math.trunc(magnitude / 1000)}K`;
  }
  return `${formatThousands(value)}c`;
}

/** 带完整千分位与正负号的金额。 */
export function formatCreditAmountFull(value: number): string {
  return formatThousands(value);
}

/** 已持有时间：天、时、分、秒只保留最高两级；负数按零处理。 */
export function formatHeldDuration(nanoseconds: number): string {
  const clamped = Math.max(0, nanoseconds);
  const seconds = Math.trunc(clamped / 1_000_000_000);
  if (seconds === 0) {
    return "0s";
  }
  const days = Math.trunc(seconds / 86_400);
  const hours = Math.trunc((seconds % 86_400) / 3_600);
  const minutes = Math.trunc((seconds % 3_600) / 60);
  if (days > 0) {
    return `${days}d ${String(hours).padStart(2, "0")}h ${String(minutes).padStart(2, "0")}m`;
  }
  if (hours > 0) {
    return `${hours}h ${String(minutes).padStart(2, "0")}m`;
  }
  if (minutes > 0) {
    return `${minutes}m`;
  }
  return `${seconds}s`;
}

/** ppm 百分比：正数保留加号；负号使用数学减号；最多四位小数并去尾零。 */
export function formatPpm(value: number, showPositive: boolean): string {
  const magnitude = Math.round(Math.abs(value));
  const whole = Math.trunc(magnitude / 10_000);
  const fraction = String(magnitude % 10_000)
    .padStart(4, "0")
    .replace(/0+$/, "");
  const decimal = fraction.length > 0 ? `.${fraction}` : "";
  const sign = value < 0 ? "\u2212" : value > 0 && showPositive ? "+" : "";
  return `${sign}${whole}${decimal}%`;
}

/** 按给定分母缩放金额，小数固定三位后去尾零。 */
function formatScaledAmount(value: number, scale: number, suffix: string): string {
  const magnitude = Math.abs(value);
  const whole = Math.trunc(magnitude / scale);
  const fraction = String(Math.trunc((magnitude % scale) / (scale / 1000)))
    .padStart(3, "0")
    .replace(/0+$/, "");
  const decimal = fraction.length > 0 ? `.${fraction}` : "";
  const sign = value < 0 ? "-" : "";
  return `${sign}${whole}${decimal}${suffix}`;
}

/** 逐位加千分位，避免本地化分组分隔符随语言变化。 */
function formatThousands(value: number): string {
  const magnitude = Math.abs(value);
  const digits = String(magnitude);
  let formatted = "";
  for (let index = 0; index < digits.length; index += 1) {
    if (index > 0 && (digits.length - index) % 3 === 0) {
      formatted += ",";
    }
    formatted += digits[index];
  }
  const sign = value < 0 ? "-" : "";
  return `${sign}${formatted}`;
}
