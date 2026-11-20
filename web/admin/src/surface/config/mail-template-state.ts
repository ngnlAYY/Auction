// 邮件模板卡片的草稿、确认与服务端冲突状态；命令结果未知时只允许原样重试。

import type { AdminMailTemplateDto } from "@auction/contracts/dto/admin/mail-templates";
import { type Cell, createCell } from "@auction/web-shared/reactive/cell";
import type { TaskResource } from "@auction/web-shared/reactive/resource";
import { err, ok, type Result } from "@auction/web-shared/result";
import { t } from "@auction/web-shared/ui/i18n";
import { onCleanup } from "solid-js";

import {
  type MailTemplateAttempt,
  type MailTemplateBodyProblem,
  mailTemplateBodyProblem,
  prepareMailTemplateAttemptOnce,
  submitMailTemplate,
} from "../../api/notification-templates";
import type { AdminSessionHost } from "../../api/session";
import { commandDeliveryReported, type PendingCommands, trySetIfAlive } from "../pending";
import { usePendingCommands } from "../pending/sources";
import { templateLabel } from "./mail-template-labels";

export const TEMPLATE_ENTITY_PREFIX = "mail-template:";

function templateEntity(key: string): string {
  return `${TEMPLATE_ENTITY_PREFIX}${key}`;
}

interface TemplateDraft {
  readonly revision: number;
  readonly body: string;
}

/** 冻结的确认输入：模板键、预期修订与正文；正文为 `null` 表示恢复内置默认。 */
export interface TemplateConfirmation {
  readonly key: string;
  readonly expectedRevision: number;
  readonly body: string | null;
}

type TemplateListResult = Result<readonly AdminMailTemplateDto[], string>;
/** 列表结果携带读取世代；只有世代与当前刷新计数一致时才允许提交。 */
export type TemplateListRead = readonly [generation: bigint, result: TemplateListResult];

/** 卡片持有尚未提交的草稿和确认，已提交的模板意图归认证会话。 */
export interface TemplateEditState {
  readonly pending: PendingCommands;
  readonly confirmation: Cell<TemplateConfirmation | null>;
  /** 正文与首次编辑时的版本一起保留，列表刷新不得替换草稿的版本围栏。 */
  readonly drafts: Cell<ReadonlyMap<string, TemplateDraft>>;
  /** 服务端冲突按模板保留，避免另一模板的操作清除这一次拒绝。 */
  readonly conflicts: Cell<ReadonlySet<string>>;
  /** 最近一次被数据库接受的模板键，用于显示成功提示。 */
  readonly accepted: Cell<string | null>;
  readonly snapshot: TaskResource<TemplateListRead>;
  readonly refresh: Cell<bigint>;
  readonly alive: () => boolean;

  listReady(): boolean;
  currentRevision(key: string): number | null;
  canEdit(key: string, revision: number): boolean;
  beginSave(template: AdminMailTemplateDto): MailTemplateBodyProblem | null;
  submit(confirmation: TemplateConfirmation): void;
  discard(key: string, conflicted: boolean): void;
}

interface LifetimeToken {
  active(): boolean;
}

/** 组件卸载后仍可能收到命令回执；写局部反馈前检查自身 owner 是否仍存活。 */
function createLifetimeToken(): LifetimeToken {
  let active = true;
  onCleanup(() => {
    active = false;
  });
  return { active: () => active };
}

function saveRevision(draft: TemplateDraft, published: number, rejected: boolean): number | null {
  return !rejected && draft.revision === published ? draft.revision : null;
}

function sameConfirmation(left: TemplateConfirmation | null, right: TemplateConfirmation): boolean {
  return (
    left !== null &&
    left.key === right.key &&
    left.expectedRevision === right.expectedRevision &&
    left.body === right.body
  );
}

/** 创建一次卡片挂载的状态；模板读取与成功提示共享同一份状态。 */
export function useTemplateEditState(
  host: AdminSessionHost,
  snapshot: TaskResource<TemplateListRead>,
  refresh: Cell<bigint>,
): TemplateEditState {
  const alive = createLifetimeToken().active;
  const pending = usePendingCommands(refresh, (entity) =>
    entity.startsWith(TEMPLATE_ENTITY_PREFIX),
  );
  const confirmation = createCell<TemplateConfirmation | null>(null);
  const drafts = createCell<ReadonlyMap<string, TemplateDraft>>(new Map());
  const conflicts = createCell<ReadonlySet<string>>(new Set());
  const accepted = createCell<string | null>(null);

  const state: TemplateEditState = {
    pending,
    confirmation,
    drafts,
    conflicts,
    accepted,
    snapshot,
    refresh,
    alive,

    listReady() {
      const generation = refresh();
      const loaded = snapshot.peek();
      // 命令了结立即使旧版本失效，Resource 尚未开始新一轮读取时也不能提交。
      return snapshot.state() === "ready" && loaded !== undefined && loaded[0] === generation;
    },

    currentRevision(key) {
      const loaded = snapshot.peek();
      if (loaded === undefined || !loaded[1].ok) {
        return null;
      }
      return loaded[1].value.find((template) => template.key === key)?.revision ?? null;
    },

    canEdit(key, revision) {
      return (
        !pending.contains(templateEntity(key)) &&
        !pending.containsLocal(TEMPLATE_ENTITY_PREFIX) &&
        confirmation.peek() === null &&
        !conflicts.peek().has(key) &&
        state.currentRevision(key) === revision
      );
    },

    beginSave(template) {
      const key = template.key;
      if (!state.canEdit(key, template.revision) || !state.listReady()) {
        return null;
      }
      const draft = drafts.peek().get(key);
      if (draft === undefined) {
        return null;
      }
      const revision = saveRevision(draft, template.revision, false);
      if (revision === null) {
        return null;
      }
      const problem = mailTemplateBodyProblem(template, draft.body);
      if (problem !== null) {
        return problem;
      }
      accepted.set(null);
      confirmation.set({
        key,
        expectedRevision: revision,
        body: draft.body,
      });
      return null;
    },

    submit(submitted) {
      if (
        !sameConfirmation(confirmation.peek(), submitted) ||
        pending.containsLocal(TEMPLATE_ENTITY_PREFIX) ||
        conflicts.peek().has(submitted.key) ||
        !state.listReady() ||
        state.currentRevision(submitted.key) !== submitted.expectedRevision
      ) {
        return;
      }
      const key = submitted.key;
      const entity = templateEntity(key);
      const actionWord =
        submitted.body !== null ? t("admin-mail-template-save") : t("admin-mail-template-reset");
      const action = `${actionWord} · ${templateLabel(key)}`;
      // 执行体首次运行时才结合 command ID 生成正文；重试只复用确认的版本与正文。
      let prepare: ReturnType<typeof prepareMailTemplateAttemptOnce> | null = null;
      const begun = pending.submitWithReceipt(
        entity,
        action,
        async (commandId, signal) => {
          if (prepare === null) {
            const attempt: MailTemplateAttempt = {
              commandId,
              key: submitted.key,
              expectedRevision: submitted.expectedRevision,
              body: submitted.body,
            };
            prepare = prepareMailTemplateAttemptOnce(attempt);
          }
          const prepared = prepare(commandId);
          if (!prepared.ok) {
            return err(prepared.error.code);
          }
          try {
            const result = await submitMailTemplate(host, prepared.value, signal);
            return result.ok ? ok(result.value) : err(result.error.code);
          } catch {
            // 异常原文可能含秘密，按结果未知交给未决命令宿主。
            return err("network_error");
          }
        },
        (template, sourceAlive) => {
          const displayed = sourceAlive && trySetIfAlive(accepted, template.key, alive);
          if (displayed) {
            drafts.update((current) => {
              const next = new Map(current);
              next.delete(template.key);
              return next;
            });
            // 接受后的旧投影不能成为下一份草稿的基准。
            snapshot.clear();
          }
          return commandDeliveryReported(displayed, () => ({
            kind: "text",
            text: t("admin-mail-template-done"),
          }));
        },
        (code) => {
          if (code === "mail_template_conflict") {
            // 账本核对和修订围栏使旧命令了结，未保存草稿仍保留原版本。
            conflicts.update((current) => new Set([...current, key]));
          }
        },
      );
      if (begun) {
        confirmation.set(null);
      }
    },

    discard(key, conflicted) {
      if (pending.contains(templateEntity(key)) || pending.containsLocal(TEMPLATE_ENTITY_PREFIX)) {
        return;
      }
      if (confirmation.peek()?.key === key) {
        confirmation.set(null);
      }
      drafts.update((current) => {
        const next = new Map(current);
        next.delete(key);
        return next;
      });
      conflicts.update((current) => {
        const next = new Set(current);
        next.delete(key);
        return next;
      });
      if (conflicted) {
        snapshot.clear();
        snapshot.restart();
      }
    },
  };
  return state;
}
