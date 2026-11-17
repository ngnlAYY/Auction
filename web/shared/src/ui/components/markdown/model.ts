// Markdown 解析结果的内部模型。

/** 行内节点；链接与图片只携带已校验的目标。 */
export type Inline =
  | { readonly kind: "text"; readonly value: string }
  | { readonly kind: "strong"; readonly nodes: readonly Inline[] }
  | { readonly kind: "emphasis"; readonly nodes: readonly Inline[] }
  | { readonly kind: "strike"; readonly nodes: readonly Inline[] }
  | { readonly kind: "code"; readonly value: string }
  | { readonly kind: "link"; readonly label: readonly Inline[]; readonly target: string }
  | { readonly kind: "image"; readonly alt: string; readonly source: string };

/** 块级节点；列表与表格保留已解析的行内内容。 */
export type MarkdownBlock =
  | { readonly kind: "heading"; readonly level: number; readonly nodes: readonly Inline[] }
  | { readonly kind: "paragraph"; readonly nodes: readonly Inline[] }
  | { readonly kind: "unordered_list"; readonly items: readonly (readonly Inline[])[] }
  | {
      readonly kind: "ordered_list";
      readonly start: number;
      readonly items: readonly (readonly Inline[])[];
    }
  | { readonly kind: "quote"; readonly nodes: readonly Inline[] }
  | {
      readonly kind: "table";
      readonly headers: readonly (readonly Inline[])[];
      readonly rows: readonly (readonly (readonly Inline[])[])[];
    }
  | { readonly kind: "code"; readonly language: string | null; readonly value: string }
  | { readonly kind: "rule" };
