// 外壳按已确认的身份持有未结竞价、回执和请求任务。

import { type Cell, createCell } from "@auction/web-shared/reactive/cell";
import { createTaskScope } from "@auction/web-shared/reactive/tasks";
import { err } from "@auction/web-shared/result";
import { createContext, createRoot, onCleanup, untrack } from "solid-js";
import { estimatedNow } from "../../api/clock";
import { COMMAND_NOT_SENT, commandNotSent } from "../../api/commands";
import type { PublicSessionResource } from "../index";
import {
  type CommandPayload,
  type CommandReceipt,
  type CommandTarget,
  commandBusy,
  outcomeUnknown,
  type SentCommand,
  sendCommand,
} from "./request";

/** 外壳可同步撤换身份宿主；暂时复核不经过这个入口。 */
export interface CompetitionCommands {
  readonly current: Cell<CompetitionStore | null>;
  replaceIdentity(userId: string | null): void;
}

/** 页面只访问外壳的竞价宿主。 */
export const CompetitionCommandsContext = createContext<CompetitionCommands | null>(null);

/** 在身份证据的处理栈中丢弃旧宿主和任务，无需等待界面重渲染。 */
export function createCompetitionCommands(
  session: PublicSessionResource,
  refresh: Cell<number>,
): CompetitionCommands {
  const current = createCell<CompetitionStore | null>(null);
  let release: (() => void) | null = null;
  onCleanup(() => release?.());
  return {
    current,
    replaceIdentity(userId) {
      release?.();
      release = null;
      current.set(
        userId === null
          ? null
          : createRoot((dispose) => {
              release = dispose;
              const canSubmit = () => {
                const snapshot = session();
                return (
                  session.state() === "ready" &&
                  snapshot?.ok === true &&
                  snapshot.value?.user_id === userId
                );
              };
              return createStore(userId, canSubmit, refresh);
            }),
      );
    },
  };
}

/** 以不透明的弹窗标识交付回执。宿主不持有页面回调。 */
export type CommandDialogOrigin = symbol;

/** 命令保留到取得明确结果或玩家确认放弃。 */
type CompetitionStore = ReturnType<typeof createStore>;

interface State {
  readonly pending: ReadonlyMap<string, SentCommand>;
  readonly receipts: readonly CommandReceipt[];
  readonly latest: CommandReceipt | null;
  readonly dialog: {
    readonly origin: CommandDialogOrigin;
    readonly receipt: CommandReceipt;
  } | null;
}

function createStore(userId: string, sessionReady: () => boolean, refresh: Cell<number>) {
  const tasks = createTaskScope();
  const canSubmit = () => tasks.alive() && sessionReady();
  const state = createCell<State>({ pending: new Map(), receipts: [], latest: null, dialog: null });
  let activeDialog: CommandDialogOrigin | null = null;
  const showReceipt = (previous: State, receipt: CommandReceipt): State => ({
    ...previous,
    receipts: [...previous.receipts, receipt],
    latest: receipt,
  });
  const setPending = (command: SentCommand) => {
    state.update((previous) => ({
      ...previous,
      pending: new Map(previous.pending).set(command.target.lotId, command),
    }));
  };
  const send = (command: SentCommand, origin: CommandDialogOrigin | null, retry: boolean) => {
    if (origin !== null) {
      activeDialog = origin;
    }
    tasks.spawn(async (context) => {
      // 确认转交后才发送。若同身份在这期间进入复核，首次请求须明确回显未发出。
      await Promise.resolve();
      if (!context.alive()) return;
      const sent = untrack(canSubmit);
      const attempt =
        command.sentAt === null && sent ? { ...command, sentAt: estimatedNow() } : command;
      if (attempt !== command) setPending(attempt);
      const result = sent ? await sendCommand(attempt, context.signal) : err(COMMAND_NOT_SENT);
      if (!context.alive()) return;
      const error = result.ok ? null : result.error;
      const notSent = error !== null && commandNotSent(error);
      if (!notSent) {
        // API 已失效命令前的读快照；存活的视图按外壳代数重读，回执不参与投影。
        refresh.update((value) => Math.min(value + 1, Number.MAX_SAFE_INTEGER));
      }
      if (error !== null && outcomeUnknown(error, retry)) {
        setPending({
          ...attempt,
          status: {
            kind: "unknown",
            retrying: false,
            error: notSent && command.status.kind === "unknown" ? command.status.error : error,
          },
        });
        return;
      }
      const receipt = {
        commandId: command.payload.commandId,
        target: command.target,
        kind: command.payload.kind,
        error,
      };
      state.update((previous) => {
        const pending = new Map(previous.pending);
        pending.delete(command.target.lotId);
        const next = { ...previous, pending };
        return origin !== null && origin === activeDialog
          ? { ...next, dialog: { origin, receipt } }
          : showReceipt(next, receipt);
      });
    });
  };
  return {
    canSubmit,
    command: (lotId: string) => state().pending.get(lotId) ?? null,
    unknownCommands: () =>
      [...state().pending.values()]
        .filter(({ status }) => status.kind === "unknown")
        .sort((left, right) => (left.sentAt ?? 0) - (right.sentAt ?? 0)),
    receipts: () => state().receipts,
    latestReceipt: () => state().latest,
    submit(target: CommandTarget, payload: CommandPayload, origin: CommandDialogOrigin): boolean {
      // 同拍品的三类命令共用门禁，排队的旧表单事件也不能建立另一条记录。
      const current = state.peek();
      if (
        !untrack(canSubmit) ||
        payload.userId !== userId ||
        current.pending.has(target.lotId) ||
        current.dialog !== null
      )
        return false;
      const command: SentCommand = {
        target: { ...target },
        payload: { ...payload },
        sentAt: null,
        status: { kind: "submitting" },
      };
      setPending(command);
      send(command, origin, false);
      return true;
    },
    retry(lotId: string, commandId: string, origin: CommandDialogOrigin | null): void {
      const command = state.peek().pending.get(lotId);
      if (
        !untrack(canSubmit) ||
        command?.payload.commandId !== commandId ||
        command.status.kind !== "unknown" ||
        command.status.retrying
      )
        return;
      const retrying: SentCommand = { ...command, status: { ...command.status, retrying: true } };
      setPending(retrying);
      send(retrying, origin, true);
    },
    abandon(lotId: string, commandId: string): void {
      const previous = state.peek();
      const command = previous.pending.get(lotId);
      if (command?.payload.commandId !== commandId || commandBusy(command)) return;
      // 放弃只清理本地恢复记录；不会撤回、重放或替换已发出的命令。
      const pending = new Map(previous.pending);
      pending.delete(lotId);
      state.set({ ...previous, pending });
    },
    dialogReceipt(origin: CommandDialogOrigin): CommandReceipt | null {
      const dialog = state().dialog;
      return dialog?.origin === origin ? dialog.receipt : null;
    },
    takeDialogReceipt(origin: CommandDialogOrigin): CommandReceipt | null {
      const previous = state.peek();
      if (previous.dialog?.origin !== origin) return null;
      state.set({ ...previous, dialog: null });
      return previous.dialog.receipt;
    },
    detach(origin: CommandDialogOrigin): void {
      if (activeDialog !== origin) return;
      activeDialog = null;
      const previous = state.peek();
      if (previous.dialog?.origin === origin) {
        // 回执与路由变更落在同一轮时，仍由外壳回显一次。
        state.set(showReceipt({ ...previous, dialog: null }, previous.dialog.receipt));
      }
    },
    dismiss(commandId: string): void {
      state.update((previous) => ({
        ...previous,
        receipts: previous.receipts.filter((receipt) => receipt.commandId !== commandId),
        latest: previous.latest?.commandId === commandId ? null : previous.latest,
      }));
    },
  };
}
