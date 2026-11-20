// 规则 Markdown 预览组件。

import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { MarkdownBlocks, withoutLeadingTitle } from "@auction/web-shared/ui/components/markdown";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { Show } from "solid-js";

import type { AdminRulesCandidate } from "../../api/rules";

/** 展示已进入当前编辑状态的规则草稿，不执行发布或远程写入。 */
export function AdminRulesPreview(props: { readonly draft: AdminRulesCandidate }): JSX.Element {
  const markdown = createValueMemo(() => withoutLeadingTitle(props.draft.markdown));
  const empty = createValueMemo(() => rustTrim(markdown()).length === 0);
  const title = createValueMemo(() => rustTrim(props.draft.title));
  const label = createValueMemo(() => (title().length === 0 ? t("admin-rules-untitled") : title()));
  return (
    <div class="admin-rules-preview">
      <h2>{t("admin-rules-preview")}</h2>
      <article aria-label={label()} class="shared-markdown">
        <MarkdownBlocks markdown={markdown()} />
      </article>
      <Show when={empty()}>
        <p>{t("admin-rules-empty-preview")}</p>
      </Show>
    </div>
  );
}

/** 与 Rust `str::trim` 的字符集合一致的裁剪。 */
function rustTrim(value: string): string {
  let start = 0;
  let end = value.length;
  while (start < end && isRustWhitespace(value.charCodeAt(start))) {
    start += 1;
  }
  while (end > start && isRustWhitespace(value.charCodeAt(end - 1))) {
    end -= 1;
  }
  return value.slice(start, end);
}

function isRustWhitespace(code: number): boolean {
  return (
    (code >= 0x09 && code <= 0x0d) ||
    code === 0x20 ||
    code === 0x85 ||
    code === 0xa0 ||
    code === 0x1680 ||
    (code >= 0x2000 && code <= 0x200a) ||
    code === 0x2028 ||
    code === 0x2029 ||
    code === 0x202f ||
    code === 0x205f ||
    code === 0x3000
  );
}
