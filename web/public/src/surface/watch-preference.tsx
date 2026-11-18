//! 账号警告渠道的独立命令宿主；只持有内存意图，不与竞价分页资源共用生命周期。
//! MM 与 PM 是两个独立开关：可以都开，也可以都关（标记只作收藏，不产生提醒）。

import type {
  PublicLotWatchPreferenceResponse,
  PublicSetLotWatchPreferenceRequest,
} from "@auction/contracts/dto/public/watch-preference";
import { createUuidV4 } from "@auction/web-shared/platform/uuid";
import type { Cell } from "@auction/web-shared/reactive/cell";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import type { TaskScope } from "@auction/web-shared/reactive/tasks";
import { createTaskScope } from "@auction/web-shared/reactive/tasks";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { createEffect, Show, untrack, useContext } from "solid-js";

import type { CommandError } from "../api/commands";
import { commandErrorOutcomeUncertain } from "../api/commands";
import {
  channelFlags,
  channelValue,
  fetchLotWatchPreference,
  setLotWatchPreference,
} from "../api/watch-preference";
import {
  PublicCommandOwnerContext,
  type PublicSessionResource,
  PublicSessionResourceContext,
} from ".";
import { useContentLoading } from "./loading";

interface PreferenceState {
  authoritative: PublicLotWatchPreferenceResponse | null;
  selected: string | null;
  pending: PublicSetLotWatchPreferenceRequest | null;
  pendingUncertain: boolean;
  busy: boolean;
  error: CommandError | null;
  confirmed: boolean;
}

interface PreferenceOwner {
  userId: string;
  /** 外壳提供的只读当前身份；命令处理里直接取当前值，不复制到异步更新的镜像。 */
  identity: () => string | null;
  session: PublicSessionResource;
  state: Cell<PreferenceState>;
  /** 组件宿主创建的任务集合；事件处理器不直接取上下文。 */
  tasks: TaskScope;
}

const DISABLED_FLAGS: readonly [boolean, boolean] = [false, false];

function preferenceStateDefault(): PreferenceState {
  return {
    authoritative: null,
    selected: null,
    pending: null,
    pendingUncertain: false,
    busy: false,
    error: null,
    confirmed: false,
  };
}

function preferenceOwnerIdentityMatches(owner: PreferenceOwner): boolean {
  return untrack(owner.identity) === owner.userId;
}

/** 会话暂时失败不能清除已提交意图，但也不能授权新的渠道变更。 */
function preferenceOwnerSessionReady(owner: PreferenceOwner): boolean {
  if (!preferenceOwnerIdentityMatches(owner) || owner.session.state() !== "ready") {
    return false;
  }
  const snapshot = owner.session();
  if (snapshot === undefined || !snapshot.ok || snapshot.value === null) {
    return false;
  }
  const identity = snapshot.value;
  return identity.authenticated && identity.user_id === owner.userId;
}

/** 只有首次读取、确定性失败后的显式刷新或已确认写入才重读；未知命令不得丢弃。 */
function preferenceOwnerLoad(owner: PreferenceOwner, confirmed: boolean): void {
  if (!preferenceOwnerIdentityMatches(owner)) {
    return;
  }
  if (!confirmed) {
    const current = owner.state.peek();
    if (current.busy || current.pendingUncertain) {
      return;
    }
  }
  owner.state.set({ ...preferenceStateDefault(), busy: true });
  owner.tasks.spawn(async (context) => {
    if (!preferenceOwnerIdentityMatches(owner)) {
      return;
    }
    const result = await fetchLotWatchPreference(owner.userId, context.signal);
    if (!context.alive() || !preferenceOwnerIdentityMatches(owner)) {
      return;
    }
    owner.state.update((current) => {
      if (result.ok) {
        const preference = result.value;
        return {
          ...current,
          busy: false,
          selected: preference.channel,
          authoritative: preference,
          confirmed,
        };
      }
      return { ...current, busy: false, error: result.error };
    });
  });
}

/** 未知命令不允许刷新后抛弃；明确成功后重读，避免把幂等历史结果当作最新账号偏好。 */
function preferenceOwnerSubmit(owner: PreferenceOwner): void {
  if (!preferenceOwnerIdentityMatches(owner) || owner.state.peek().busy) {
    return;
  }
  const current = owner.state.peek();
  let payload: PublicSetLotWatchPreferenceRequest;
  if (current.pending !== null) {
    if (!current.pendingUncertain) {
      return;
    }
    payload = { ...current.pending };
  } else {
    if (!preferenceOwnerSessionReady(owner)) {
      return;
    }
    const preference = current.authoritative;
    const selected = current.selected;
    if (preference === null || selected === null) {
      return;
    }
    payload = {
      command_id: createUuidV4(),
      channel: selected,
      expected_version: preference.version,
    };
  }
  owner.state.set({
    ...owner.state.peek(),
    pending: payload,
    busy: true,
    error: null,
    confirmed: false,
  });
  owner.tasks.spawn(async (context) => {
    if (!preferenceOwnerIdentityMatches(owner)) {
      return;
    }
    const result = await setLotWatchPreference(owner.userId, payload, context.signal);
    if (!context.alive() || !preferenceOwnerIdentityMatches(owner)) {
      return;
    }
    if (result.ok) {
      // 幂等重放可以返回历史结果，必须 GET 后才展示当前渠道及下一次 CAS 版本。
      preferenceOwnerLoad(owner, true);
      return;
    }
    const error = result.error;
    owner.state.update((state) => {
      // 服务端先查账本再校验版本和私信资格，只有对应的明确拒绝才能结束首次未知。
      const rejected =
        (error.status === 409 && error.code === "watch_preference_conflict") ||
        pmChannelUnavailable(error);
      const pendingUncertain =
        !rejected && (state.pendingUncertain || commandErrorOutcomeUncertain(error));
      return { ...state, busy: false, pendingUncertain, error };
    });
  });
}

/** 父账户组件按 user_id 设置 key；卸载取消所属任务，跨身份响应还需前后双重检查。 */
export function OutbidNotifications(props: { readonly userId: string }): JSX.Element {
  const session = useContext(PublicSessionResourceContext);
  const identitySource = useContext(PublicCommandOwnerContext);
  const state = createCell<PreferenceState>(preferenceStateDefault());
  const tasks = createTaskScope();
  const owner: PreferenceOwner = {
    userId: props.userId,
    identity: identitySource,
    session,
    state,
    tasks,
  };
  createEffect(
    () => null,
    () => {
      preferenceOwnerLoad(owner, false);
    },
  );
  const snapshot = () => state();
  const busy = () => snapshot().busy;
  const pending = () => snapshot().pending !== null;
  const ready = () => preferenceOwnerSessionReady(owner) && snapshot().authoritative !== null;
  // 读取偏好由顶栏提示；保存是本表单的命令反馈，留在表单内。
  useContentLoading(() => busy() && !pending());
  const failed = () => snapshot().error !== null;
  const uncertain = () => snapshot().pendingUncertain;
  const message = () => feedback(snapshot());
  const flags = createValueMemo(() => channelFlags(snapshot().selected ?? "") ?? DISABLED_FLAGS);
  const mm = () => flags()[0];
  const pm = () => flags()[1];
  // PM 需要论坛同步已启用且账号已绑定论坛账号；不可用时不允许新开启，已开启的仍可关闭。
  const pmAvailable = () => snapshot().authoritative?.pm_available === true;
  const confirmed = () => snapshot().confirmed;
  const submit = () => preferenceOwnerSubmit(owner);
  const retry = () => preferenceOwnerSubmit(owner);
  const refresh = () => {
    if (!state.peek().busy) {
      preferenceOwnerLoad(owner, false);
    }
  };
  // 两个开关各自只改写本渠道，另一渠道保持当前选择；提交仍是一条完整的偏好命令。
  const toggle = (isPm: boolean, checked: boolean) => {
    if (!preferenceOwnerSessionReady(owner) || state.peek().busy || state.peek().pending !== null) {
      return;
    }
    const current = state.peek();
    const [mmCurrent, pmCurrent] = channelFlags(current.selected ?? "") ?? DISABLED_FLAGS;
    const next = isPm ? channelValue(mmCurrent, checked) : channelValue(checked, pmCurrent);
    state.set({ ...current, selected: next, confirmed: false });
  };

  return (
    <section
      id="outbid-notifications"
      class="public-outbid-preference"
      aria-busy={busy() ? "true" : "false"}
    >
      <fieldset aria-describedby="outbid-feedback">
        <legend>{t("outbid-title")}</legend>
        <div class="public-outbid-preference__choices">
          <div>
            <input
              id="outbid-channel-mm"
              type="checkbox"
              name="outbid-channel-mm"
              checked={mm()}
              disabled={!ready() || busy() || pending()}
              aria-describedby="outbid-feedback"
              onChange={(event) => toggle(false, event.currentTarget.checked)}
            />
            <label for="outbid-channel-mm">{t("outbid-mm")}</label>
          </div>
          <div>
            <input
              id="outbid-channel-pm"
              type="checkbox"
              name="outbid-channel-pm"
              checked={pm()}
              disabled={!ready() || busy() || pending() || (!pmAvailable() && !pm())}
              aria-describedby="outbid-feedback outbid-channel-hint"
              onChange={(event) => toggle(true, event.currentTarget.checked)}
            />
            <label for="outbid-channel-pm">{t("outbid-pm")}</label>
          </div>
        </div>
        {/* 渠道说明常驻同一位置：都未选时说明不会收到提醒，PM 不可用时说明原因。 */}
        <div id="outbid-channel-hint" class="public-outbid-preference__hint">
          <Show when={ready() && !mm() && !pm()}>
            <p>{t("outbid-none-hint")}</p>
          </Show>
          <Show when={ready() && !pmAvailable()}>
            <p>{t("outbid-pm-unavailable")}</p>
          </Show>
        </div>
        <div class="public-outbid-preference__actions">
          <button
            type="button"
            disabled={!ready() || busy() || pending()}
            aria-describedby="outbid-feedback"
            onClick={submit}
          >
            {t("outbid-save")}
          </button>
          <Show
            when={failed() && uncertain()}
            fallback={
              <Show when={failed()}>
                <button
                  type="button"
                  disabled={busy()}
                  aria-describedby="outbid-feedback"
                  onClick={refresh}
                >
                  {t("outbid-refresh")}
                </button>
              </Show>
            }
          >
            <button
              type="button"
              disabled={busy()}
              aria-describedby="outbid-feedback"
              onClick={retry}
            >
              {t("outbid-retry-command")}
            </button>
          </Show>
        </div>
        <div id="outbid-feedback" aria-live="polite" aria-atomic="true">
          <Show
            when={failed()}
            fallback={
              <Show
                when={busy() && pending()}
                fallback={<Show when={confirmed()}>{<p>{t("outbid-confirmed")}</p>}</Show>}
              >
                <p>{t("outbid-saving")}</p>
              </Show>
            }
          >
            <p class="public-outbid-preference__error">{message()}</p>
          </Show>
        </div>
      </fieldset>
    </section>
  );
}

function pmChannelUnavailable(error: CommandError | null): boolean {
  return error?.status === 422 && error.code === "pm_channel_unavailable";
}

function feedback(state: PreferenceState): string {
  if (state.pending === null) {
    return t("outbid-load-error");
  }
  if (state.error?.status === 409 && state.error.code === "watch_preference_conflict") {
    return t("outbid-conflict");
  }
  if (state.pendingUncertain) {
    return t("outbid-unknown");
  }
  if (pmChannelUnavailable(state.error)) {
    return t("outbid-pm-unavailable");
  }
  return t("outbid-save-error");
}
