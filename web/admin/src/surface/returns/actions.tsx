// 结算退件详情页的处理动作按服务端允许的动作显示按钮，确认后交给会话命令宿主。
//
// 确认框只展示服务端冻结的后果（原收件人、资产与补寄次数），不提供可编辑的资产、金额或收件人。
// 提交时冻结案件、版本、动作与 `command_id`。结果未知可在未决区原样重试，被明确拒绝时重读详情。

import type {
  AdminSettlementReturnDetailResponse,
  AdminSettlementReturnSummaryResponse,
} from "@auction/contracts/dto/admin/returns";
import type { Cell } from "@auction/web-shared/reactive/cell";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { err, ok } from "@auction/web-shared/result";
import { Dialog } from "@auction/web-shared/ui/components/dialog";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { For, Show } from "solid-js";

import { prepareSettlementReturnActionOnce, submitSettlementReturnAction } from "../../api/returns";
import { useAdminSessionHost } from "../auth/shell";
import { definiteRejection, requirePendingCommands, setIfAlive } from "../pending";
import { advanceCommandReload } from "../pending/sources";
import type { AdminActionFeedback } from "../presentation";
import { actionEffectKey, actionKey, blockerKey, label } from "./labels";
import { assetsText } from "./list";

/** 一次待确认的动作：冻结读取时的案件版本，确认后原样提交。 */
interface PendingAction {
  readonly action: string;
  readonly expectedVersion: number;
}

/** 详情页动作区：按钮、阻断原因与确认框。 */
export function ActionSection(props: {
  readonly detail: AdminSettlementReturnDetailResponse;
  readonly notice: Cell<AdminActionFeedback | null>;
  readonly refresh: Cell<bigint>;
  readonly loadedGeneration: bigint;
}): JSX.Element {
  const commands = requirePendingCommands();
  const confirming = createCell<PendingAction | null>(null);
  const summary = () => props.detail.summary;
  const entity = createValueMemo(() => `settlement-return:${summary().return_id}`);
  const version = createValueMemo(() => summary().case_version);
  const loadedGeneration = createValueMemo(() => props.loadedGeneration);
  // 按钮文案在渲染前取好，按钮只捕获动作名与读取时的案件版本。
  const buttons = createValueMemo(() =>
    props.detail.allowed_actions.map((action) => ({
      action,
      text: label(actionKey, action),
    })),
  );
  const blockers = createValueMemo(() =>
    props.detail.blockers.map(
      (item) => `${label(actionKey, item.action)}：${label(blockerKey, item.blocker)}`,
    ),
  );
  const canOpen = (): boolean =>
    props.refresh() === loadedGeneration() && !commands.contains(entity()) && !commands.anyBusy();

  return (
    <section
      class="admin-panel admin-settlement-return-actions"
      aria-labelledby="settlement-return-actions-title"
    >
      <h2 id="settlement-return-actions-title">{t("settlement-return-actions-title")}</h2>
      <Show
        when={buttons().length === 0}
        fallback={
          <div class="admin-settlement-return-actions__buttons">
            <For each={buttons()} keyed={(button) => button.action}>
              {(button) => (
                <button
                  class="admin-btn-primary"
                  type="button"
                  disabled={!canOpen()}
                  onClick={() => {
                    if (!canOpen()) {
                      return;
                    }
                    props.notice.set(null);
                    confirming.set({
                      action: button().action,
                      expectedVersion: version(),
                    });
                  }}
                >
                  {button().text}
                </button>
              )}
            </For>
          </div>
        }
      >
        <p class="admin-panel__note">{t("settlement-return-actions-none")}</p>
      </Show>
      <Show when={blockers().length > 0}>
        <h3>{t("settlement-return-blocked-title")}</h3>
        <ul class="admin-settlement-return-actions__blockers">
          <For each={blockers()} keyed={false}>
            {(blocker) => <li>{blocker()}</li>}
          </For>
        </ul>
      </Show>
      <Show when={confirming()}>
        {(pending) => (
          <ConfirmDialog
            summary={summary()}
            pending={pending()}
            confirming={confirming}
            notice={props.notice}
            refresh={props.refresh}
            loadedGeneration={loadedGeneration()}
          />
        )}
      </Show>
    </section>
  );
}

/** 动作确认框：只读展示冻结后果，成功或失败回执写回详情页。 */
function ConfirmDialog(props: {
  readonly summary: AdminSettlementReturnSummaryResponse;
  readonly pending: PendingAction;
  readonly confirming: Cell<PendingAction | null>;
  readonly notice: Cell<AdminActionFeedback | null>;
  readonly refresh: Cell<bigint>;
  readonly loadedGeneration: bigint;
}): JSX.Element {
  const host = useAdminSessionHost();
  const commands = requirePendingCommands();
  const summary = () => props.summary;
  const pending = () => props.pending;
  const recipient = createValueMemo(() => summary().recipient ?? t("value-empty"));
  const used = createValueMemo(() => summary().resends_used ?? 0);
  const effect = createValueMemo(() => {
    const action = pending().action;
    const key = actionEffectKey(action);
    return key === null
      ? action
      : t(key, {
          recipient: recipient(),
          used: String(used()),
          limit: String(summary().resend_limit),
        });
  });
  const title = createValueMemo(() => label(actionKey, pending().action));
  const expectedVersion = createValueMemo(() => pending().expectedVersion);
  const entity = createValueMemo(() => `settlement-return:${summary().return_id}`);
  const loadedGeneration = createValueMemo(() => props.loadedGeneration);
  const sameVersion = createValueMemo(() => expectedVersion() === summary().case_version);
  const assets = createValueMemo(() => assetsText(summary()));
  // 确认绑定打开时的案件版本；读取世代切换后旧点击不能分配新的命令 ID。
  const canSubmit = (): boolean =>
    sameVersion() && props.refresh() === loadedGeneration() && !commands.anyBusy();
  const blocked = (): boolean => !canSubmit() || commands.contains(entity());

  const submit = (): void => {
    if (!canSubmit() || commands.contains(entity())) {
      return;
    }
    const returnId = summary().return_id;
    const frozenEntity = `settlement-return:${returnId}`;
    // 提交时捕获反馈句柄；来源页仍登记而传入 Cell 被替换时，旧命令不能写进新 Cell。
    const notice = props.notice;
    const refresh = props.refresh;
    // 逐字段复制并冻结，执行体与重试都不再读取确认框草稿；路由、正文与回执期望同源。
    const submission = prepareSettlementReturnActionOnce({
      returnId,
      expectedVersion: pending().expectedVersion,
      action: pending().action,
    });
    const submitted = commands.submit(
      frozenEntity,
      label(actionKey, submission.intent.action),
      async (commandId, signal) => {
        const result = await submitSettlementReturnAction(host, commandId, submission, signal);
        return result.ok ? ok(result.value) : err(result.error.code);
      },
      () => {
        // 命令宿主只在来源详情页仍存活时调用回执；notice 就属于该页。动作区可能因重读
        // 先卸载，但回执仍须保留，因此这里不叠加动作区自身的卸载判定。
        setIfAlive(notice, { kind: "success" }, () => true);
      },
      (code) => {
        // 明确拒绝（版本已变、状态或资格不符）说明案件已不同于页面所见：重读后再决定；
        // 结果未知不重读，原 command_id 留在未决区原样重试。
        if (definiteRejection(code)) {
          advanceCommandReload(refresh);
        }
        setIfAlive(notice, { kind: "error", code }, () => true);
      },
    );
    if (submitted) {
      props.confirming.set(null);
      notice.set(null);
    }
  };

  return (
    <Dialog
      title={title()}
      closeLabel={t("settlement-return-cancel")}
      busy={commands.anyBusy()}
      onClose={() => props.confirming.set(null)}
    >
      <p>{effect()}</p>
      <dl class="admin-delivery-detail__grid">
        <dt>{t("settlement-return-column-recipient")}</dt>
        <dd>{recipient()}</dd>
        <dt>{t("settlement-return-column-assets")}</dt>
        <dd>{assets()}</dd>
        <dt>{t("settlement-return-field-version")}</dt>
        <dd>{expectedVersion()}</dd>
      </dl>
      <div class="admin-shell__dialog-actions">
        <button
          class="admin-btn-secondary"
          type="button"
          disabled={commands.anyBusy()}
          autofocus
          onClick={() => props.confirming.set(null)}
        >
          {t("settlement-return-cancel")}
        </button>
        <button class="admin-btn-primary" type="button" disabled={blocked()} onClick={submit}>
          {t("settlement-return-confirm")}
        </button>
      </div>
    </Dialog>
  );
}
