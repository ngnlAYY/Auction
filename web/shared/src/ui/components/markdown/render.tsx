// Markdown 结构到安全 DOM 的渲染。

import type { JSX } from "@solidjs/web";
import { createEffect, For, Match, onSettled, Switch } from "solid-js";

import { createCell } from "../../../reactive/cell";
import { createValueMemo } from "../../../reactive/memo";
import { unreachable } from "../../../result";
import { t } from "../../i18n";
import { TableColumns } from "../table-columns";
import { isInternalTarget } from "./inline";
import type { Inline, MarkdownBlock } from "./model";

const ALPHANUMERIC = /[\p{Alphabetic}\p{N}]/u;

type HeadingBlock = Extract<MarkdownBlock, { readonly kind: "heading" }>;
type ParagraphBlock = Extract<MarkdownBlock, { readonly kind: "paragraph" }>;
type UnorderedListBlock = Extract<MarkdownBlock, { readonly kind: "unordered_list" }>;
type OrderedListBlock = Extract<MarkdownBlock, { readonly kind: "ordered_list" }>;
type QuoteBlock = Extract<MarkdownBlock, { readonly kind: "quote" }>;
type TableBlock = Extract<MarkdownBlock, { readonly kind: "table" }>;
type CodeBlock = Extract<MarkdownBlock, { readonly kind: "code" }>;
type TextInline = Extract<Inline, { readonly kind: "text" }>;
type StrongInline = Extract<Inline, { readonly kind: "strong" }>;
type EmphasisInline = Extract<Inline, { readonly kind: "emphasis" }>;
type StrikeInline = Extract<Inline, { readonly kind: "strike" }>;
type CodeInline = Extract<Inline, { readonly kind: "code" }>;
type LinkInline = Extract<Inline, { readonly kind: "link" }>;
type ImageInline = Extract<Inline, { readonly kind: "image" }>;

/** 把单个块级节点渲染为对应的安全 DOM 结构。 */
export function MarkdownBlockView(props: { readonly block: MarkdownBlock }): JSX.Element {
  return (
    <Switch>
      <Match when={asHeadingBlock(props.block)}>
        {(block) => <MarkdownHeading block={block()} />}
      </Match>
      <Match when={asParagraphBlock(props.block)}>
        {(block) => (
          <p>
            <MarkdownInline nodes={block().nodes} />
          </p>
        )}
      </Match>
      <Match when={asUnorderedListBlock(props.block)}>
        {(block) => <MarkdownUnorderedList block={block()} />}
      </Match>
      <Match when={asOrderedListBlock(props.block)}>
        {(block) => <MarkdownOrderedList block={block()} />}
      </Match>
      <Match when={asQuoteBlock(props.block)}>
        {(block) => (
          <blockquote>
            <MarkdownInline nodes={block().nodes} />
          </blockquote>
        )}
      </Match>
      <Match when={asTableBlock(props.block)}>{(block) => <MarkdownTable block={block()} />}</Match>
      <Match when={asCodeBlock(props.block)}>{(block) => <MarkdownCode block={block()} />}</Match>
      <Match when={props.block.kind === "rule"}>
        <hr />
      </Match>
    </Switch>
  );
}

/**
 * 文档表格没有固定业务字段：按表头与前 32 行的典型可见长度（第 75 百分位、CJK 双宽）
 * 估算列权重，避免单条长 URL/说明挤占整张表。
 */
function tableColumnWeights(
  headers: readonly (readonly Inline[])[],
  rows: readonly (readonly (readonly Inline[])[])[],
): number[] {
  return headers.map((header, index) => {
    // 采样最多 32 行，固定缓冲避免每列分配临时数组。
    const samples = new Array<number>(32).fill(0);
    let count = 0;
    let sampleIndex = 0;
    for (const row of rows.slice(0, samples.length)) {
      const nodes = row[index];
      if (nodes === undefined) {
        continue;
      }
      samples[sampleIndex] = visibleUnits(nodes);
      sampleIndex += 1;
      count = sampleIndex;
    }
    const lengths = samples.slice(0, count).sort((left, right) => left - right);
    const typical = lengths[Math.floor((lengths.length * 3) / 4)] ?? 0;
    return Math.min(Math.max(Math.max(visibleUnits(header), typical), 8), 48);
  });
}

function visibleUnits(nodes: readonly Inline[]): number {
  return countVisibleUnits(nodes, { value: 96 });
}

/** 列宽只采样前 96 个可见字符；直接遍历节点，避免先复制完整单元格再丢弃长后缀。 */
function countVisibleUnits(nodes: readonly Inline[], remaining: { value: number }): number {
  let units = 0;
  for (const node of nodes) {
    if (remaining.value === 0) {
      break;
    }
    switch (node.kind) {
      case "text":
      case "code":
        units += countCharacters(node.value, remaining);
        break;
      case "image":
        units += countCharacters(node.alt, remaining);
        break;
      case "strong":
      case "emphasis":
      case "strike":
        units += countVisibleUnits(node.nodes, remaining);
        break;
      case "link":
        units += countVisibleUnits(node.label, remaining);
        break;
      default:
        unreachable(node);
    }
  }
  return units;
}

function countCharacters(value: string, remaining: { value: number }): number {
  let units = 0;
  for (const character of value) {
    if (remaining.value === 0) {
      break;
    }
    const codePoint = character.codePointAt(0) ?? 0;
    units += codePoint <= 0x7f ? 1 : 2;
    remaining.value -= 1;
  }
  return units;
}

function MarkdownHeading(props: { readonly block: HeadingBlock }): JSX.Element {
  // 标题生成 id 锚点，供目录或站内 # 链接跳转；无效 slug（如纯标点标题）则不生成 id。
  const anchor = createValueMemo(() => headingAnchor(props.block.nodes));
  const nodes = (): readonly Inline[] => props.block.nodes;
  return (
    <Switch
      fallback={
        <h6 id={anchor() ?? undefined}>
          <MarkdownInline nodes={nodes()} />
        </h6>
      }
    >
      <Match when={props.block.level === 1}>
        <h1 id={anchor() ?? undefined}>
          <MarkdownInline nodes={nodes()} />
        </h1>
      </Match>
      <Match when={props.block.level === 2}>
        <h2 id={anchor() ?? undefined}>
          <MarkdownInline nodes={nodes()} />
        </h2>
      </Match>
      <Match when={props.block.level === 3}>
        <h3 id={anchor() ?? undefined}>
          <MarkdownInline nodes={nodes()} />
        </h3>
      </Match>
      <Match when={props.block.level === 4}>
        <h4 id={anchor() ?? undefined}>
          <MarkdownInline nodes={nodes()} />
        </h4>
      </Match>
      <Match when={props.block.level === 5}>
        <h5 id={anchor() ?? undefined}>
          <MarkdownInline nodes={nodes()} />
        </h5>
      </Match>
    </Switch>
  );
}

/**
 * 从标题行内节点提取纯文本并 slug 化为锚点 id；仅保留字母数字（含 CJK），其余折叠为
 * 单个连字符。文本为空或全是标点时返回 `null`，不生成空 id。
 */
function headingAnchor(nodes: readonly Inline[]): string | null {
  const text = { value: "" };
  collectText(nodes, text);

  let slug = "";
  for (const character of rustTrim(text.value)) {
    if (ALPHANUMERIC.test(character)) {
      slug += asciiLowercase(character);
    } else if (slug.length > 0 && !slug.endsWith("-")) {
      slug += "-";
    }
  }
  return slug.length > 0 ? slug : null;
}

/** 递归收集行内节点的可见纯文本，供锚点和表格滚动区域命名。 */
function collectText(nodes: readonly Inline[], output: { value: string }): void {
  for (const node of nodes) {
    switch (node.kind) {
      case "text":
      case "code":
        output.value += node.value;
        break;
      case "strong":
      case "emphasis":
      case "strike":
        collectText(node.nodes, output);
        break;
      case "link":
        collectText(node.label, output);
        break;
      case "image":
        output.value += node.alt;
        break;
      default:
        unreachable(node);
    }
  }
}

function MarkdownInline(props: { readonly nodes: readonly Inline[] }): JSX.Element {
  return (
    <For each={props.nodes} keyed={false}>
      {(node) => <MarkdownInlineView node={node()} />}
    </For>
  );
}

function MarkdownInlineView(props: { readonly node: Inline }): JSX.Element {
  return (
    <Switch>
      <Match when={asTextInline(props.node)}>{(node) => <span>{node().value}</span>}</Match>
      <Match when={asStrongInline(props.node)}>
        {(node) => (
          <strong>
            <MarkdownInline nodes={node().nodes} />
          </strong>
        )}
      </Match>
      <Match when={asEmphasisInline(props.node)}>
        {(node) => (
          <em>
            <MarkdownInline nodes={node().nodes} />
          </em>
        )}
      </Match>
      <Match when={asStrikeInline(props.node)}>
        {(node) => (
          <del>
            <MarkdownInline nodes={node().nodes} />
          </del>
        )}
      </Match>
      <Match when={asCodeInline(props.node)}>
        {(node) => <code translate="no">{node().value}</code>}
      </Match>
      <Match when={asLinkInline(props.node)}>{(node) => <MarkdownLink node={node()} />}</Match>
      <Match when={asImageInline(props.node)}>
        {(node) => <img src={node().source} alt={node().alt} loading="lazy" />}
      </Match>
    </Switch>
  );
}

function MarkdownLink(props: { readonly node: LinkInline }): JSX.Element {
  // 站内链接在当前页跳转，避免整页刷新打断 SPA；外链才开新页并加 noreferrer。
  const internal = createValueMemo(() => isInternalTarget(props.node.target));
  return (
    <a
      href={props.node.target}
      target={internal() ? undefined : "_blank"}
      rel={internal() ? undefined : "noreferrer"}
    >
      <MarkdownInline nodes={props.node.label} />
    </a>
  );
}

function MarkdownUnorderedList(props: { readonly block: UnorderedListBlock }): JSX.Element {
  return (
    <ul>
      <For each={props.block.items} keyed={false}>
        {(nodes) => (
          <li>
            <MarkdownInline nodes={nodes()} />
          </li>
        )}
      </For>
    </ul>
  );
}

function MarkdownOrderedList(props: { readonly block: OrderedListBlock }): JSX.Element {
  return (
    <ol start={props.block.start.toString()}>
      <For each={props.block.items} keyed={false}>
        {(nodes) => (
          <li>
            <MarkdownInline nodes={nodes()} />
          </li>
        )}
      </For>
    </ol>
  );
}

function MarkdownTable(props: { readonly block: TableBlock }): JSX.Element {
  const widths = createValueMemo(() => tableColumnWeights(props.block.headers, props.block.rows));
  const scrollable = createCell(false);
  let wrapper: HTMLDivElement | undefined;

  const regionName = createValueMemo<string | null>(() => {
    if (!scrollable()) return null;
    const headers = props.block.headers.map((nodes) => {
      const text = { value: "" };
      collectText(nodes, text);
      return rustTrim(text.value);
    });
    return headers.filter(Boolean).join(", ") || t("markdown-table-scroll-region");
  });

  onSettled(() => {
    const element = wrapper;
    if (element === undefined) return;
    const measure = () => {
      // 桌面 display: contents 和只裁剪内容的容器都不应进入 Tab 顺序。
      const overflowX = window.getComputedStyle(element).overflowX;
      scrollable.set(
        element.clientWidth > 0 &&
          element.scrollWidth > element.clientWidth &&
          (overflowX === "auto" || overflowX === "scroll"),
      );
    };
    measure();
    const observer = typeof ResizeObserver === "function" ? new ResizeObserver(measure) : null;
    observer?.observe(element);
    if (element.firstElementChild !== null) observer?.observe(element.firstElementChild);
    window.addEventListener("resize", measure);
    return () => {
      observer?.disconnect();
      window.removeEventListener("resize", measure);
    };
  });

  createEffect(regionName, (name) => {
    if (wrapper === undefined) return;
    if (name === null) {
      wrapper.removeAttribute("tabindex");
      wrapper.removeAttribute("role");
      wrapper.removeAttribute("aria-label");
    } else {
      wrapper.setAttribute("tabindex", "0");
      wrapper.setAttribute("role", "region");
      wrapper.setAttribute("aria-label", name);
    }
  });

  return (
    <div
      class="shared-markdown__table-wrap"
      ref={(element) => {
        wrapper = element;
      }}
    >
      <table>
        <TableColumns layout="markdown-content" widths={widths()} />
        <thead>
          <tr>
            <For each={props.block.headers} keyed={false}>
              {(nodes) => (
                <th>
                  <MarkdownInline nodes={nodes()} />
                </th>
              )}
            </For>
          </tr>
        </thead>
        <tbody>
          <For each={props.block.rows} keyed={false}>
            {(row) => (
              <tr>
                <For each={row()} keyed={false}>
                  {(nodes) => (
                    <td>
                      <MarkdownInline nodes={nodes()} />
                    </td>
                  )}
                </For>
              </tr>
            )}
          </For>
        </tbody>
      </table>
    </div>
  );
}

function MarkdownCode(props: { readonly block: CodeBlock }): JSX.Element {
  return (
    <pre translate="no">
      <Switch fallback={<code>{props.block.value}</code>}>
        <Match when={props.block.language !== null}>
          <code class={`language-${props.block.language ?? ""}`}>{props.block.value}</code>
        </Match>
      </Switch>
    </pre>
  );
}

function asHeadingBlock(block: MarkdownBlock): HeadingBlock | null {
  return block.kind === "heading" ? block : null;
}

function asParagraphBlock(block: MarkdownBlock): ParagraphBlock | null {
  return block.kind === "paragraph" ? block : null;
}

function asUnorderedListBlock(block: MarkdownBlock): UnorderedListBlock | null {
  return block.kind === "unordered_list" ? block : null;
}

function asOrderedListBlock(block: MarkdownBlock): OrderedListBlock | null {
  return block.kind === "ordered_list" ? block : null;
}

function asQuoteBlock(block: MarkdownBlock): QuoteBlock | null {
  return block.kind === "quote" ? block : null;
}

function asTableBlock(block: MarkdownBlock): TableBlock | null {
  return block.kind === "table" ? block : null;
}

function asCodeBlock(block: MarkdownBlock): CodeBlock | null {
  return block.kind === "code" ? block : null;
}

function asTextInline(node: Inline): TextInline | null {
  return node.kind === "text" ? node : null;
}

function asStrongInline(node: Inline): StrongInline | null {
  return node.kind === "strong" ? node : null;
}

function asEmphasisInline(node: Inline): EmphasisInline | null {
  return node.kind === "emphasis" ? node : null;
}

function asStrikeInline(node: Inline): StrikeInline | null {
  return node.kind === "strike" ? node : null;
}

function asCodeInline(node: Inline): CodeInline | null {
  return node.kind === "code" ? node : null;
}

function asLinkInline(node: Inline): LinkInline | null {
  return node.kind === "link" ? node : null;
}

function asImageInline(node: Inline): ImageInline | null {
  return node.kind === "image" ? node : null;
}

function asciiLowercase(character: string): string {
  const code = character.codePointAt(0) ?? 0;
  return code >= 65 && code <= 90 ? String.fromCharCode(code + 32) : character;
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
