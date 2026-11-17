// 表格列比例的渲染原语；字段顺序与具体比例由各业务表格拥有。

import type { JSX } from "@solidjs/web";
import { For } from "solid-js";

import { createValueMemo } from "../../reactive/memo";

/** `layout` 是调用方的表格布局标识，`widths` 是各列权重。 */
export interface TableColumnsProps {
  readonly layout: string;
  readonly widths: readonly number[];
}

/** 按调用方给定的列权重生成 colgroup，不推断业务字段，也不套用全站等分比例。
 * 业务表通常传入总和为 100 的百分比，Markdown 传入按内容长度估算的权重；
 * 归一化后列布局始终受 100% 容器约束。 */
export function TableColumns(props: TableColumnsProps): JSX.Element {
  const total = createValueMemo(() =>
    Math.max(
      1,
      props.widths.reduce((sum, width) => sum + width, 0),
    ),
  );
  return (
    <colgroup data-table-layout={props.layout}>
      <For each={props.widths} keyed={false}>
        {(width) => <col style={`width: ${formatTableColumnPercent(width(), total())}%`} />}
      </For>
    </colgroup>
  );
}

function formatTableColumnPercent(width: number, total: number): string {
  return formatFixedSix((width * 100) / total);
}

// Rust 的 `{:.6}` 按精确十进制值舍入，恰好半位时取偶数；`toFixed` 在半位时远离零，
// 会在 0.0078125 一类列宽上产生不同的 HTML。
function formatFixedSix(value: number): string {
  const negative = value < 0;
  const magnitude = Math.abs(value);
  const view = new DataView(new ArrayBuffer(8));
  view.setFloat64(0, magnitude, false);
  const bits = view.getBigUint64(0, false);
  const exponentBits = Number((bits >> 52n) & 0x7ffn);
  const fraction = bits & 0xfffffffffffffn;
  const significand = exponentBits === 0 ? fraction : (1n << 52n) | fraction;
  const exponent = exponentBits === 0 ? -1074 : exponentBits - 1023 - 52;
  let numerator = significand * 1_000_000n;
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
  const digits = rounded.toString().padStart(7, "0");
  return `${negative ? "-" : ""}${digits.slice(0, -6)}.${digits.slice(-6)}`;
}
