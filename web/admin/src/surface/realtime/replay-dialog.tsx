// Realtime failure 的 replay 对话框：冻结提交输入，并把结果交给会话命令宿主。

import type { Cell } from "@auction/web-shared/reactive/cell";
import { Dialog } from "@auction/web-shared/ui/components/dialog";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { Show } from "solid-js";

import {
  prepareRealtimeReplay,
  type RealtimeFailure,
  replayRealtimeFailure,
} from "../../api/realtime";
import type { AdminSessionHost } from "../../api/session";
import { type PendingCommands, setIfAlive } from "../pending";
import { type AdminActionFeedback, adminApiErrorMessage } from "../presentation";

const I32_MIN_MAGNITUDE = 2_147_483_648;
const I32_MAX = 2_147_483_647;

// 登记命令前的试算 UUID：`prepareRealtimeReplay` 只有在调用返回的工厂时才给出校验结果；
// 这里用合法的非 nil 值取一次结果并丢弃试算正文，真正的正文仍按宿主分配的 command_id 生成。
const PREPARATION_PROBE_COMMAND_ID = "00000000-0000-4000-8000-000000000001";

/**
 * Replay 对话框的页面接线：schema 与 payload 草稿、结果反馈和页面存活判定都由页面持有，
 * 组件只读写这些句柄，并在登记命令前用与执行体相同的准备函数校验草稿。
 */
export interface RealtimeReplayDialogProps {
  readonly failure: RealtimeFailure;
  readonly host: AdminSessionHost;
  readonly pending: PendingCommands;
  readonly notice: Cell<AdminActionFeedback | null>;
  readonly schemaVersion: Cell<string>;
  readonly publicPayload: Cell<string>;
  readonly replayInvalid: Cell<boolean>;
  readonly busy: () => boolean;
  readonly available: () => boolean;
  readonly processed: boolean;
  readonly readError: string | null;
  readonly readPending: boolean;
  readonly onRetryRead: () => void;
  readonly pageAlive: () => boolean;
  readonly onClose: () => void;
  readonly onSubmitted: () => void;
}

/** Replay 对话框；目标、schema 与 payload 在提交时冻结，执行体不读取草稿。 */
export function RealtimeReplayDialog(props: RealtimeReplayDialogProps): JSX.Element {
  const busy = (): boolean => props.busy();
  const submitDisabled = (): boolean => busy() || !props.available();
  const submitReplay = (): void => {
    const { id, event_id, stream_key } = props.failure;
    if (submitDisabled() || props.pending.contains(`realtime-failure:${id}`)) {
      return;
    }
    const schemaInput = props.schemaVersion();
    const payload = props.publicPayload();
    const schema = parseI32(schemaInput);
    if (schema === null) {
      props.replayInvalid.set(true);
      return;
    }
    if (schema <= 0) {
      props.replayInvalid.set(true);
      return;
    }
    // 与 Rust 一样在登记命令前完成草稿校验：准备函数覆盖 serde_json 的解析边界
    // （127 层容器、重复键、无法原样往返的数字）与正文冻结。失败时保留对话框与草稿，
    // 不登记命令、不发请求；成功时再由新的准备函数为实际 command_id 生成正文。
    if (!prepareRealtimeReplay(schema, payload)(PREPARATION_PROBE_COMMAND_ID).ok) {
      props.replayInvalid.set(true);
      return;
    }
    props.replayInvalid.set(false);
    props.notice.set(null);
    // 已登记的命令只使用冻结目标、schema 与负载，原样重试无需读列表。
    const failure = { id, event_id, stream_key };
    const prepare = prepareRealtimeReplay(schema, payload);
    const submitted = props.pending.submit(
      `realtime-failure:${id}`,
      t("realtime-replay-submit"),
      (commandId, signal) => replayRealtimeFailure(props.host, failure, commandId, prepare, signal),
      () => {
        setIfAlive(props.notice, { kind: "success" }, props.pageAlive);
      },
      (code) => {
        setIfAlive(props.notice, { kind: "error", code }, props.pageAlive);
      },
    );
    if (submitted) {
      props.onSubmitted();
    }
  };

  return (
    // 外壳（遮罩/标题栏/关闭守卫）由共享 Dialog 接管；.admin-realtime-dialog
    // 内容样式挂在面板内的包装 div；共享组件暂不支持 aria_describedby。
    <Dialog
      title={t("realtime-replay-title", { id: props.failure.id })}
      closeLabel={t("realtime-replay-cancel")}
      busy={busy()}
      onClose={props.onClose}
    >
      <div class="admin-realtime-dialog">
        <p class="admin-panel__note">
          {t("realtime-replay-warning", { event_id: props.failure.event_id })}
        </p>
        <div role="status">
          <Show when={props.processed}>
            <p class="admin-panel__note">{t("realtime-replay-processed")}</p>
          </Show>
        </div>
        <div role="alert">
          <Show when={props.readError}>
            {(code) => (
              <p class="admin-panel__note action-feedback action-feedback--error">
                {adminApiErrorMessage(code())}
              </p>
            )}
          </Show>
        </div>
        <Show when={props.readError !== null}>
          <button
            type="button"
            class="admin-btn-secondary"
            disabled={props.readPending}
            onClick={() => props.onRetryRead()}
          >
            {t("admin-retry")}
          </button>
        </Show>
        <label>
          {t("realtime-replay-schema")}
          <input
            type="number"
            min="1"
            step="1"
            autofocus={true}
            disabled={busy()}
            value={props.schemaVersion()}
            onInput={(event) => {
              if (busy()) {
                return;
              }
              props.schemaVersion.set(event.currentTarget.value);
              props.replayInvalid.set(false);
            }}
          />
        </label>
        <label>
          {t("realtime-replay-payload")}
          <textarea
            spellcheck="false"
            disabled={busy()}
            value={props.publicPayload()}
            onInput={(event) => {
              if (busy()) {
                return;
              }
              props.publicPayload.set(event.currentTarget.value);
              props.replayInvalid.set(false);
            }}
          />
        </label>
        <Show when={props.replayInvalid()}>
          <p role="alert">{t("realtime-replay-invalid")}</p>
        </Show>
        <div class="admin-shell__dialog-actions">
          <button type="button" disabled={submitDisabled()} onClick={submitReplay}>
            {t("realtime-replay-submit")}
          </button>
          <button
            type="button"
            class="admin-btn-secondary"
            disabled={busy()}
            onClick={props.onClose}
          >
            {t("realtime-replay-cancel")}
          </button>
        </div>
      </div>
    </Dialog>
  );
}

/** Rust `str::parse::<i32>`：可选正负号、仅 ASCII 十进制数字，且必须落在 i32 范围。 */
function parseI32(text: string): number | null {
  let index = 0;
  let negative = false;
  const first = text.charAt(0);
  if (first === "+" || first === "-") {
    negative = first === "-";
    index = 1;
  }
  if (index === text.length) {
    return null;
  }
  let magnitude = 0;
  for (; index < text.length; index += 1) {
    const code = text.charCodeAt(index);
    if (code < 0x30 || code > 0x39) {
      return null;
    }
    magnitude = magnitude * 10 + (code - 0x30);
    if (magnitude > I32_MIN_MAGNITUDE) {
      return null;
    }
  }
  if (negative) {
    return -magnitude;
  }
  return magnitude > I32_MAX ? null : magnitude;
}
