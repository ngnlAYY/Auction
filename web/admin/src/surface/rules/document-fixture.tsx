// 规则页面测试的会话级草稿与可卸载路由分支；发布仍走真实工作区按钮。

import { createCell } from "@auction/web-shared/reactive/cell";
import type { JSX } from "@solidjs/web";
import { Show } from "solid-js";

import type { AdminDocumentKind, AdminRules, AdminRulesCandidate } from "../../api/rules";
import { createCommandReload, usePendingCommands } from "../pending/sources";
import {
  DocumentDraftsContext,
  documentDraftsGet,
  type RuleNotice,
  type RulesBaseline,
  rulesBaselineFromDocument,
  useRulesDraft,
} from "./model";
import { page } from "./page";
import { AdminRulesWorkspace } from "./workspace";

/** 按 Rust 夹具模拟离开和返回，草稿与命令宿主始终保留在路由外面。 */
export function DocumentHarness(props: { readonly kind: AdminDocumentKind }): JSX.Element {
  const drafts = useRulesDraft();
  const open = createCell(true);
  return (
    <DocumentDraftsContext value={drafts}>
      <button type="button" onClick={() => open.update((value) => !value)}>
        切换测试路由
      </button>
      <button
        type="button"
        onClick={() => {
          const current = documentDraftsGet(drafts, props.kind);
          const candidate = { locale: "en", title: "新标题", markdown: "新正文" };
          current.draft.set(candidate);
          current.checkedCandidate.set(candidate);
        }}
      >
        准备已校验草稿
      </button>
      <Show when={open()}>{page(props.kind)}</Show>
    </DocumentDraftsContext>
  );
}

/** 用 Rust 工作区测试的原文档和已校验草稿挂载真实工作区。 */
export function WorkspaceHarness(): JSX.Element {
  const document: AdminRules = {
    locale: "zh-CN",
    title: "原规则",
    markdown: "原正文",
    published_by: "operator",
    revision: 1,
    published_at: "2026-11-18T00:00:00Z",
  };
  const candidate: AdminRulesCandidate = {
    locale: document.locale,
    title: document.title,
    markdown: "更新的正文",
  };
  const baseline = createCell<RulesBaseline | null>(rulesBaselineFromDocument(document));
  const draft = createCell<AdminRulesCandidate | null>(candidate);
  const checkedCandidate = createCell<AdminRulesCandidate | null>(candidate);
  const target = createCell(document.locale);
  const notice = createCell<RuleNotice | null>(null);
  const busy = createCell(false);
  const reloadGeneration = createCommandReload(0n);
  const pending = usePendingCommands(reloadGeneration, () => true);

  return (
    <AdminRulesWorkspace
      kind="rules"
      document={document}
      target={target()}
      targetSignal={target}
      draft={draft}
      checkedCandidate={checkedCandidate}
      baseline={baseline}
      notice={notice}
      busy={busy}
      reloadGeneration={reloadGeneration}
      pending={pending}
      dirty={() => true}
    />
  );
}
