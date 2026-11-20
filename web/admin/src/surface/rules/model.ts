// 会话内站点文档（规则、关于）草稿、版本基线和已提交发布意图；两种文档各有一份，互不影响。

import { type Cell, createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { unreachable } from "@auction/web-shared/result";
import type { Accessor } from "solid-js";
import { createContext } from "solid-js";

import type { AdminDocumentKind, AdminRules, AdminRulesCandidate } from "../../api/rules";
import { createCommandReload } from "../pending/sources";
import { useUnloadWarning } from "../presentation";
import { DEFAULT_RULES_LOCALE } from "./draft";

/** 编辑器反馈；错误只保存稳定码，文案由当前语言映射。 */
export type RuleNotice =
  | { readonly kind: "validated" }
  | { readonly kind: "published" }
  | { readonly kind: "action-error"; readonly code: string }
  | { readonly kind: "conflict" };

/** 丢弃确认只保存用户选择的下一步，不承载发布命令或草稿正文。 */
export type RulesTransition =
  | { readonly kind: "select"; readonly locale: string }
  | { readonly kind: "reload" };

/** 草稿所属的权威版本；比较正文而不依赖可碰撞的摘要。 */
export interface RulesBaseline {
  readonly locale: string;
  readonly revision: number;
  readonly title: string;
  readonly markdown: string;
}

export function rulesBaselineFromDocument(document: AdminRules): RulesBaseline {
  return {
    locale: document.locale,
    revision: document.revision,
    title: document.title,
    markdown: document.markdown,
  };
}

export function rulesBaselineMatchesDocument(
  baseline: RulesBaseline,
  document: AdminRules,
): boolean {
  return (
    baseline.locale === document.locale &&
    baseline.revision === document.revision &&
    baseline.title === document.title &&
    baseline.markdown === document.markdown
  );
}

export function rulesBaselineMatchesCandidate(
  baseline: RulesBaseline,
  candidate: AdminRulesCandidate,
): boolean {
  return (
    baseline.locale === candidate.locale &&
    baseline.title === candidate.title &&
    baseline.markdown === candidate.markdown
  );
}

export function rulesBaselineMatchesLocale(baseline: RulesBaseline, locale: string): boolean {
  return baseline.locale === locale;
}

/** 当前认证身份独占草稿、校验状态和已提交发布意图，站内路由不持有它们。 */
export interface RulesDraft {
  readonly target: Cell<string>;
  readonly draft: Cell<AdminRulesCandidate | null>;
  readonly baseline: Cell<RulesBaseline | null>;
  readonly checkedCandidate: Cell<AdminRulesCandidate | null>;
  readonly notice: Cell<RuleNotice | null>;
  readonly busy: Cell<boolean>;
  readonly reloadGeneration: Cell<bigint>;
  readonly dirty: Accessor<boolean>;
}

/** 两种文档的草稿 owner；页面按自己的文档种类取用。 */
export interface DocumentDrafts {
  readonly rules: RulesDraft;
  readonly about: RulesDraft;
}

export function documentDraftsGet(drafts: DocumentDrafts, kind: AdminDocumentKind): RulesDraft {
  switch (kind) {
    case "rules":
      return drafts.rules;
    case "about":
      return drafts.about;
    default:
      return unreachable(kind);
  }
}

/** 规则与关于草稿的上下文；缺少提供方是装配错误。 */
export const DocumentDraftsContext = createContext<DocumentDrafts | null>(null);

/** 身份变化销毁 owner；站内导航只卸载读取组件，不取消已提交命令。 */
export function useRulesDraft(): DocumentDrafts {
  const rules = useDocumentDraft();
  const about = useDocumentDraft();
  const drafts: DocumentDrafts = { rules, about };
  // 任一文档有未发布的修改都要在离开页面前提醒。
  const dirty = createValueMemo(() => rules.dirty() || about.dirty());
  useUnloadWarning(dirty);
  return drafts;
}

function useDocumentDraft(): RulesDraft {
  const target = createCell(DEFAULT_RULES_LOCALE);
  const draft = createCell<AdminRulesCandidate | null>(null);
  const baseline = createCell<RulesBaseline | null>(null);
  const checkedCandidate = createCell<AdminRulesCandidate | null>(null);
  const notice = createCell<RuleNotice | null>(null);
  const busy = createCell(false);
  const reloadGeneration = createCommandReload(0n);
  const dirty = createValueMemo(() => {
    const candidate = draft();
    const base = baseline();
    if (candidate === null) {
      return false;
    }
    return base === null || !rulesBaselineMatchesCandidate(base, candidate);
  });
  return {
    target,
    draft,
    baseline,
    checkedCandidate,
    notice,
    busy,
    reloadGeneration,
    dirty,
  };
}
