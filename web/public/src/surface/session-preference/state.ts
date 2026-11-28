// 登录时长设置的账号隔离命令宿主；未知结果冻结命令、时长和 CAS 版本直到明确确认。

import type {
  PublicSessionPreferenceResponse,
  PublicSetSessionPreferenceRequest,
} from "@auction/contracts/dto/public/account";
import { createUuidV4 } from "@auction/web-shared/platform/uuid";
import type { Cell } from "@auction/web-shared/reactive/cell";
import { structuralEquals } from "@auction/web-shared/reactive/equality";
import type { TaskScope } from "@auction/web-shared/reactive/tasks";
import { untrack } from "solid-js";

import {
  type CommandError,
  commandErrorOutcomeUncertain,
  commandNotSent,
} from "../../api/commands";
import { fetchSessionPreference, setSessionPreference } from "../../api/session-preference";
import {
  currentSessionConfirmation,
  type PublicSessionAuthority,
} from "../shell/session-authority";
import {
  type DurationDraft,
  durationDraftDefault,
  durationDraftFromSeconds,
  durationDraftSeconds,
} from "./duration";

/** `Option<Option<String>>` 的浏览器表示：null 表示尚未取得会话证明，permanent 表示明确无截止。 */
export type CurrentSessionExpiry =
  | { readonly kind: "finite"; readonly expiresAt: string }
  | { readonly kind: "permanent" };

export interface PreferenceState {
  authoritative: PublicSessionPreferenceResponse | null;
  draft: DurationDraft;
  currentExpiresAt: CurrentSessionExpiry | null;
  pending: PublicSetSessionPreferenceRequest | null;
  pendingUncertain: boolean;
  busy: boolean;
  error: CommandError | null;
  validationError: boolean;
  confirmed: boolean;
}

export interface PreferenceOwner {
  userId: string;
  /** 外壳提供的只读当前身份；读取在响应式作用域内订阅变化，在命令处理里直接取当前值。 */
  identity: () => string | null;
  session: PublicSessionAuthority;
  state: Cell<PreferenceState>;
  /** 组件宿主创建的任务集合；事件处理器不直接取上下文。 */
  tasks: TaskScope;
  readStarted(): void;
}

export function preferenceStateDefault(): PreferenceState {
  return {
    authoritative: null,
    draft: durationDraftDefault(),
    currentExpiresAt: null,
    pending: null,
    pendingUncertain: false,
    busy: false,
    error: null,
    validationError: false,
    confirmed: false,
  };
}

export function preferenceOwnerIdentityMatches(owner: PreferenceOwner): boolean {
  return untrack(owner.identity) === owner.userId;
}

/** 修改输入和发送命令须有当前已完成的会话证明。 */
export function preferenceOwnerSessionReady(owner: PreferenceOwner): boolean {
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

export function preferenceOwnerChange(
  owner: PreferenceOwner,
  update: (draft: DurationDraft) => void,
): void {
  if (!preferenceOwnerSessionReady(owner)) {
    return;
  }
  const current = owner.state.peek();
  if (current.busy || current.pending !== null || current.authoritative === null) {
    return;
  }
  const draft = { ...current.draft };
  update(draft);
  owner.state.set({
    ...current,
    draft,
    validationError: false,
    confirmed: false,
  });
}

/** 未知命令不允许刷新后抛弃；明确成功后重读，避免把幂等历史结果当作最新账号偏好。 */
export function preferenceOwnerLoad(
  owner: PreferenceOwner,
  confirmed: boolean,
  preserveDraft = false,
): void {
  if (!preferenceOwnerIdentityMatches(owner)) {
    return;
  }
  if (!confirmed) {
    const current = owner.state.peek();
    if (current.busy || current.pendingUncertain) {
      return;
    }
  }
  const current = owner.state.peek();
  owner.readStarted();
  owner.state.set(
    preserveDraft
      ? { ...current, busy: true }
      : { ...preferenceStateDefault(), busy: true, currentExpiresAt: current.currentExpiresAt },
  );
  owner.tasks.spawn(async (context) => {
    if (!preferenceOwnerIdentityMatches(owner)) {
      return;
    }
    const result = await fetchSessionPreference(owner.userId, context.signal);
    if (!context.alive() || !preferenceOwnerIdentityMatches(owner)) {
      return;
    }
    if (result.ok) {
      const preference = result.value;
      owner.state.update((current) => {
        const currentExpiresAt: CurrentSessionExpiry =
          preference.expires_at === null
            ? { kind: "permanent" }
            : { kind: "finite", expiresAt: preference.expires_at };
        const dirty =
          current.pending !== null ||
          (current.authoritative !== null &&
            !structuralEquals(
              current.draft,
              durationDraftFromSeconds(current.authoritative.duration_seconds),
            ));
        // 重新登录后的读回不能把用户草稿重新绑定到另一个 CAS 版本。
        if (preserveDraft && dirty) {
          return { ...current, busy: false, currentExpiresAt };
        }
        return {
          ...current,
          busy: false,
          draft: durationDraftFromSeconds(preference.duration_seconds),
          currentExpiresAt,
          authoritative: preference,
          error: null,
          confirmed: confirmed || current.confirmed,
        };
      });
      return;
    }
    owner.state.update((current) => ({
      ...current,
      busy: false,
      error: current.pending === null ? result.error : current.error,
    }));
  });
}

export function preferenceOwnerSubmit(owner: PreferenceOwner): void {
  if (!preferenceOwnerSessionReady(owner) || owner.state.peek().busy) {
    return;
  }
  let payload: PublicSetSessionPreferenceRequest;
  const current = owner.state.peek();
  if (current.pending !== null) {
    if (!current.pendingUncertain) {
      return;
    }
    payload = { ...current.pending };
  } else {
    const preference = current.authoritative;
    if (preference === null) {
      return;
    }
    const durationSeconds = durationDraftSeconds(current.draft);
    if (!durationSeconds.ok) {
      owner.state.set({ ...current, validationError: true });
      return;
    }
    payload = {
      command_id: createUuidV4(),
      duration_seconds: durationSeconds.value,
      expected_version: preference.version,
    };
  }

  owner.state.set({
    ...owner.state.peek(),
    pending: payload,
    busy: true,
    error: null,
    validationError: false,
    confirmed: false,
  });
  owner.tasks.spawn(async (context) => {
    if (!preferenceOwnerIdentityMatches(owner)) {
      return;
    }
    const result = await setSessionPreference(owner.userId, payload, context.signal, () =>
      untrack(
        () =>
          preferenceOwnerSessionReady(owner) && currentSessionConfirmation(owner.session) !== null,
      ),
    );
    if (!context.alive() || !preferenceOwnerIdentityMatches(owner)) {
      return;
    }
    if (result.ok) {
      preferenceOwnerLoad(owner, true);
      return;
    }
    const error = result.error;
    owner.state.update((state) => {
      if (commandNotSent(error)) {
        // 首次未发出可重新编辑；重放未发出时沿用上次未知的冻结请求与提示。
        return {
          ...state,
          busy: false,
          pending: current.pending,
          error: current.pendingUncertain ? current.error : error,
        };
      }
      // 只有路由的明确冲突码能否定本命令；裸 409 或损坏响应不能抹掉首次未知。
      const pendingUncertain =
        !(error.status === 409 && error.code === "session_preference_conflict") &&
        (state.pendingUncertain || commandErrorOutcomeUncertain(error));
      // 确定性结果（含明确冲突与等锁失败）释放冻结命令，用户可基于权威版本继续编辑。
      const pending = pendingUncertain ? state.pending : null;
      return { ...state, busy: false, pendingUncertain, pending, error };
    });
  });
}
