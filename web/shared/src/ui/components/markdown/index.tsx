// 共享 Markdown 文档组件及其解析边界。

import type { JSX } from "@solidjs/web";
import { For } from "solid-js";

import { createValueMemo } from "../../../reactive/memo";
import { parseBlocks } from "./blocks";
import { MarkdownBlockView } from "./render";

/** 完整 Markdown 文档组件的内容。 */
export interface MarkdownDocumentProps {
  readonly markdown: string;
}

/** 无外层容器的 Markdown 块组件的内容。 */
export interface MarkdownBlocksProps {
  readonly markdown: string;
}

/**
 * 将规则正文渲染为不执行原始 HTML 的 DOM。
 * 只支持旧版规则页实际使用的 Markdown/GFM 子集；文本节点由框架自动转义，链接只允许
 * HTTP(S)、站内相对路径、锚点和 mailto 目标，拒绝反斜杠与控制字符。
 * 单个链接标签/目标限 1/4 KiB，超限按文本处理；正文不因链接窗口限制而截断。
 */
export function MarkdownDocument(props: MarkdownDocumentProps): JSX.Element {
  return (
    <article class="shared-markdown">
      <MarkdownBlocks markdown={props.markdown} />
    </article>
  );
}

/** 只渲染 Markdown 的块级节点，不添加外层容器；供页面自带文档容器的场景使用。 */
export function MarkdownBlocks(props: MarkdownBlocksProps): JSX.Element {
  const blocks = createValueMemo(() => parseBlocks(props.markdown));
  return (
    <For each={blocks()} keyed={false}>
      {(block) => <MarkdownBlockView block={block()} />}
    </For>
  );
}

/** 移除正文首行的一级标题，避免页面标题与 Markdown 标题重复显示。 */
export function withoutLeadingTitle(markdown: string): string {
  const source = markdown.startsWith("\uFEFF") ? markdown.slice(1) : markdown;
  const lines = rustLines(source);
  const first = lines[0];
  if (first === undefined) {
    return "";
  }
  const isTitle = first.startsWith("#") && (first.charAt(1) === " " || first.charAt(1) === "\t");
  if (!isTitle) {
    return source;
  }
  // 保留 lines 的 CRLF 归一化与末尾换行处理，只跳过标题后的第一条空行。
  const rest = lines.slice(1);
  if (rest[0] === "") {
    rest.shift();
  }
  return rest.join("\n");
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
