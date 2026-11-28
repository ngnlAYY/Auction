// 受限权利页：无登录会话，凭定位符或游戏身份取得挑战码，核验后办理单个权利。
//
// 页面只使用权利定位符与服务端给出的权利视图，从不显示、请求或保存邮件正文；定位符与挑战码
// 只存在于本次提交的内存闭包里，不进地址、存储或日志，提交即清空输入。会话令牌在 HttpOnly
// 的 hv_right Cookie 里，脚本接触不到；会话失效（401）一律回到验权，不保留任何会话态。
// 写命令结果未知时只许原样重放同一命令（不变量 1、6）。

import { CommandUuidV4Schema } from "@auction/contracts/dto/public/consignment/command";
import { ListingWorldSchema } from "@auction/contracts/dto/public/consignment/listing-options";
import {
  type RightAction,
  RightActionSchema,
  type RightKind,
  type RightView,
} from "@auction/contracts/dto/public/consignment/rights";
import {
  ConsignmentUuidSchema,
  ListingVersionSchema,
} from "@auction/contracts/dto/public/consignment/scalar";
import { decode, strictStruct } from "@auction/contracts/wire";
import { parseJson } from "@auction/web-shared/net/wire";
import { getSessionItem, setSessionItem } from "@auction/web-shared/platform/storage";
import { createUuidV4 } from "@auction/web-shared/platform/uuid";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createTaskScope } from "@auction/web-shared/reactive/tasks";
import { ok, type Result } from "@auction/web-shared/result";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { For, onCleanup, Show } from "solid-js";
import type * as v from "valibot";

import {
  challengeRight,
  claimRight,
  fetchCurrentRight,
  fetchRightCommandReceipt,
  locateRight,
  type RightsError,
  type RightWrite,
  rightWriteRejected,
  verifyRight,
} from "../../api/consignment-rights";

const CLAIM_KEY = "hv.consignment.right.claim";
// 续办记录只含命令 ID、权利与版本、动作；不含定位符、挑战码或会话令牌。
const CLAIM_RECORD = strictStruct({
  command_id: CommandUuidV4Schema,
  right_id: ConsignmentUuidSchema,
  expected_version: ListingVersionSchema,
  action: RightActionSchema,
});
type ClaimRecord = v.InferOutput<typeof CLAIM_RECORD>;
// 本地存储失败与兄弟寄售页同码；本页没有专用译文键，按 `ERROR_KEYS` 的通用回退显示。
const STORAGE_FAILURE: RightsError = { status: 0, code: "pending_storage_unavailable" };

const KIND_KEYS = {
  continue_order: "consignment-right-kind-continue-order",
  claim_equipment: "consignment-right-kind-claim-equipment",
  claim_payment: "consignment-right-kind-claim-payment",
  withdraw_held: "consignment-right-kind-withdraw-held",
  loss_case: "consignment-right-kind-loss-case",
} satisfies Record<RightKind, string>;
const ACTION_KEYS = {
  continue_order: "consignment-right-action-continue-order",
  claim_equipment: "consignment-right-action-claim-equipment",
  claim_payment: "consignment-right-action-claim-payment",
  withdraw_held: "consignment-right-action-withdraw-held",
} satisfies Record<RightAction, string>;
const DISPOSITION_KEYS = {
  claim_accepted: "consignment-right-claim-accepted",
  already_fulfilling: "consignment-right-already-fulfilling",
  already_completed: "consignment-right-already-completed",
} as const;
const ERROR_KEYS: Readonly<Record<string, string>> = {
  challenge_invalid: "consignment-right-challenge-invalid",
  right_expired: "consignment-right-expired",
  action_not_allowed: "consignment-right-not-allowed",
  stale_version: "consignment-stale-version",
  rate_limited: "consignment-right-rate-limited",
  invalid_request: "consignment-right-invalid-request",
  command_id_conflict: "consignment-command-conflict",
  network_error: "error-network",
  request_timeout: "error-request-timeout",
  timeout: "error-request-timeout",
  invalid_response: "error-invalid-response",
  command_not_sent: "command-not-sent",
};

function loadClaim(): ClaimRecord | null {
  const raw = getSessionItem(CLAIM_KEY);
  if (raw === null || raw === "null" || raw.length > 512) return null;
  const json = parseJson(raw, { uniqueKeys: "all" });
  const record = json.ok ? decode(CLAIM_RECORD, json.value) : null;
  return record?.ok === true ? record.value : null;
}

/** 内存中冻结的一次写尝试：结果未知时只重放同一份闭包，不重新读取输入。 */
interface Attempt {
  readonly kind: RightWrite;
  /** 成功时给出要执行的状态迁移，失败原样返回错误。 */
  readonly send: (signal: AbortSignal) => Promise<Result<() => void, RightsError>>;
}

/** 受限权利页：找回、核验、当前权利与续办。 */
export function RightsPage(): JSX.Element {
  const tasks = createTaskScope();
  const challengeId = createCell<string | null>(null);
  const view = createCell<RightView | null>(null);
  const attempt = createCell<Attempt | null>(null);
  const claim = createCell<ClaimRecord | null>(loadClaim());
  const busy = createCell(false);
  const error = createCell<RightsError | null>(null);
  const notice = createCell<string | null>(null);
  const locator = createCell("");
  const identity = createCell("");
  const world = createCell<"main" | "isekai">("main");
  const code = createCell("");
  const inputs: HTMLInputElement[] = [];
  const wipe = () => {
    locator.set("");
    identity.set("");
    code.set("");
    for (const input of inputs) input.value = "";
  };
  onCleanup(wipe);

  // 会话失效：丢弃视图与未完成的核验态，回到验权；续办记录保留，重新验权后可原样续办。
  const backToVerify = () => {
    view.set(null);
    challengeId.set(null);
    attempt.set(null);
    notice.set("consignment-right-session-ended");
  };

  function run(next: Attempt) {
    busy.set(true);
    error.set(null);
    tasks.spawn(async (context) => {
      const result = await next.send(context.signal);
      if (!context.alive()) return;
      busy.set(false);
      if (result.ok) {
        attempt.set(null);
        notice.set(null);
        result.value();
      } else if (result.error.status === 401) {
        // 会话失效一律回到验权；不保留会重放旧会话的尝试。
        backToVerify();
      } else {
        // 结果未知只保留原尝试；已确认被拒则结案。
        attempt.set(rightWriteRejected(next.kind, result.error) ? null : next);
        if (next.kind === "verify" && result.error.code === "challenge_invalid") {
          challengeId.set(null);
        }
        error.set(result.error);
      }
    });
  }

  function finishClaim(disposition: keyof typeof DISPOSITION_KEYS) {
    // 不变量 1：清除失败时保留记录，避免刷新后把旧命令误作可继续的新命令；仍须给出可见反馈，
    // 否则已成功的续办停在未决面板上、按键继续被占用却没有任何说明。
    if (!setSessionItem(CLAIM_KEY, "null")) {
      error.set(STORAGE_FAILURE);
      return;
    }
    claim.set(null);
    error.set(null);
    notice.set(DISPOSITION_KEYS[disposition]);
  }

  /** 未决记录只属于当前验权得到的权利；换一份权利后不得在错误页面上结算或展示。 */
  const claimForView = () => {
    const record = claim();
    const current = view();
    return record !== null && current !== null && record.right_id === current.right_id
      ? record
      : null;
  };

  function checkClaim() {
    const record = claim.peek();
    if (record === null || record.right_id !== view.peek()?.right_id) return;
    busy.set(true);
    tasks.spawn(async (context) => {
      const result = await fetchRightCommandReceipt(record.command_id, context.signal);
      if (!context.alive()) return;
      busy.set(false);
      if (
        result.ok &&
        result.value.command_id === record.command_id &&
        result.value.right_id === record.right_id
      ) {
        finishClaim(result.value.disposition);
      } else if (!result.ok && result.error.status === 401) backToVerify();
      // 404 只表示回执尚不存在，授权原样重送；其余错误只展示。
      else if (!result.ok && result.error.status !== 404) error.set(result.error);
    });
  }

  function loadView() {
    busy.set(true);
    tasks.spawn(async (context) => {
      const result = await fetchCurrentRight(context.signal);
      if (!context.alive()) return;
      busy.set(false);
      if (result.ok) {
        view.set(result.value);
        challengeId.set(null);
        error.set(null);
        notice.set(null);
        checkClaim();
      } else if (result.error.status === 401) backToVerify();
      else error.set(result.error);
    });
  }

  function sendClaim(record: ClaimRecord) {
    busy.set(true);
    error.set(null);
    tasks.spawn(async (context) => {
      const result = await claimRight(
        {
          command_id: record.command_id,
          expected_version: record.expected_version,
          action: record.action,
        },
        context.signal,
      );
      if (!context.alive()) return;
      busy.set(false);
      if (result.ok) finishClaim(result.value.disposition);
      else if (result.error.status === 401) backToVerify();
      else {
        if (rightWriteRejected("claim", result.error) && setSessionItem(CLAIM_KEY, "null")) {
          claim.set(null);
        }
        error.set(result.error);
      }
    });
  }

  const accepted = (id: string) => () => challengeId.set(id);
  const locate = () => {
    const command = {
      command_id: createUuidV4(),
      game_identity: identity.peek(),
      world: world.peek(),
    };
    identity.set("");
    run({
      kind: "locate",
      send: async (signal) => {
        const result = await locateRight(command, signal);
        return result.ok ? ok(accepted(result.value.challenge_id)) : result;
      },
    });
  };
  const challenge = () => {
    const command = { command_id: createUuidV4(), right_locator: locator.peek() };
    wipe();
    run({
      kind: "challenge",
      send: async (signal) => {
        const result = await challengeRight(command, signal);
        return result.ok ? ok(accepted(result.value.challenge_id)) : result;
      },
    });
  };
  const verify = () => {
    const id = challengeId.peek();
    if (id === null) return;
    const command = { command_id: createUuidV4(), challenge_id: id, code: code.peek() };
    wipe();
    run({
      kind: "verify",
      send: async (signal) => {
        const result = await verifyRight(command, signal);
        return result.ok ? ok(loadView) : result;
      },
    });
  };
  const submitClaim = (current: RightView, action: RightAction) => {
    const existing = claim.peek();
    if (
      busy.peek() ||
      (existing !== null && existing.right_id === current.right_id) ||
      !current.allowed_actions.includes(action)
    )
      return;
    const record = decode(CLAIM_RECORD, {
      command_id: createUuidV4(),
      right_id: current.right_id,
      expected_version: current.right_version,
      action,
    });
    // 不变量 1：发送前写入严格白名单；无法记录时不发送不可恢复的写操作，并给出可见反馈。
    if (!record.ok) return;
    if (!setSessionItem(CLAIM_KEY, JSON.stringify(record.value))) {
      error.set(STORAGE_FAILURE);
      return;
    }
    claim.set(record.value);
    sendClaim(record.value);
  };
  const retry = () => {
    const held = attempt.peek();
    if (held !== null && !busy.peek()) run(held);
  };

  return (
    <section
      class="public-account-bids-page public-consignment-account"
      aria-label={t("consignment-right-title")}
    >
      <Show when={notice()}>{(key) => <p role="status">{t(key())}</p>}</Show>
      <Show when={error()}>
        {(current) => (
          <p role="alert">{t(ERROR_KEYS[current().code] ?? "consignment-right-unavailable")}</p>
        )}
      </Show>
      <Show when={attempt()}>
        <button type="button" disabled={busy()} onClick={retry}>
          {t("session-duration-retry")}
        </button>
      </Show>
      <Show
        when={view()}
        fallback={
          <Show
            when={challengeId() === null}
            fallback={
              <form
                class="public-consignment-account__form"
                autocomplete="off"
                onSubmit={(event) => {
                  event.preventDefault();
                  verify();
                }}
              >
                <p>{t("consignment-right-code-sent")}</p>
                <label>
                  {t("consignment-right-code-label")}
                  <input
                    type="password"
                    autocomplete="off"
                    spellcheck={false}
                    ref={(element) => void inputs.push(element)}
                    value={code()}
                    onInput={(event) => code.set(event.currentTarget.value)}
                  />
                </label>
                <button type="submit" disabled={busy() || attempt() !== null || code() === ""}>
                  {t("consignment-right-verify")}
                </button>
              </form>
            }
          >
            <form
              class="public-consignment-account__form"
              autocomplete="off"
              onSubmit={(event) => {
                event.preventDefault();
                challenge();
              }}
            >
              <label>
                {t("consignment-right-locator-label")}
                <input
                  type="password"
                  autocomplete="off"
                  spellcheck={false}
                  ref={(element) => void inputs.push(element)}
                  value={locator()}
                  onInput={(event) => locator.set(event.currentTarget.value)}
                />
              </label>
              <button type="submit" disabled={busy() || attempt() !== null || locator() === ""}>
                {t("consignment-right-locator-submit")}
              </button>
            </form>
            <form
              class="public-consignment-account__form"
              autocomplete="off"
              onSubmit={(event) => {
                event.preventDefault();
                locate();
              }}
            >
              <p>{t("consignment-right-locate-explain")}</p>
              <label>
                {t("consignment-right-identity-label")}
                <input
                  type="text"
                  autocomplete="off"
                  value={identity()}
                  onInput={(event) => identity.set(event.currentTarget.value)}
                />
              </label>
              <label>
                {t("consignment-world")}
                <select
                  name="world"
                  value={world()}
                  onChange={(event) =>
                    world.set(
                      ListingWorldSchema.options.find(
                        (option) => option === event.currentTarget.value,
                      ) ?? "main",
                    )
                  }
                >
                  <For each={ListingWorldSchema.options}>
                    {(option) => (
                      <option value={option}>
                        {t(option === "main" ? "auction-world-main" : "auction-world-isekai")}
                      </option>
                    )}
                  </For>
                </select>
              </label>
              <button type="submit" disabled={busy() || attempt() !== null || identity() === ""}>
                {t("consignment-right-locate-submit")}
              </button>
            </form>
          </Show>
        }
      >
        {(current) => (
          <section class="public-consignment-account__detail">
            <h2>{t(KIND_KEYS[current().kind])}</h2>
            <p>{t(current().world === "main" ? "auction-world-main" : "auction-world-isekai")}</p>
            <Show when={current().kind === "loss_case"}>
              <p>{t("consignment-right-read-only")}</p>
            </Show>
            <Show when={claimForView()}>
              <p role="status">{t("consignment-pending")}</p>
              <div class="public-consignment-account__actions">
                <button type="button" disabled={busy()} onClick={checkClaim}>
                  {t("data-retry")}
                </button>
                <button
                  type="button"
                  disabled={busy()}
                  onClick={() => {
                    const record = claimForView();
                    if (record !== null) sendClaim(record);
                  }}
                >
                  {t("session-duration-retry")}
                </button>
              </div>
            </Show>
            <div class="public-consignment-account__actions">
              <For each={current().allowed_actions}>
                {(action) => (
                  <button
                    type="button"
                    disabled={busy() || claimForView() !== null}
                    onClick={() => submitClaim(current(), action)}
                  >
                    {t(ACTION_KEYS[action])}
                  </button>
                )}
              </For>
            </div>
          </section>
        )}
      </Show>
    </section>
  );
}
