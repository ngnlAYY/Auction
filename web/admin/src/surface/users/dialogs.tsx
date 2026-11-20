// Admin 用户操作对话框的标题、动作区与命令交付。

import type { Cell } from "@auction/web-shared/reactive/cell";
import { unreachable } from "@auction/web-shared/result";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";

import { adminApiFailureDisplayCode } from "../../api/failure";
import type { AdminUserStatusFilter } from "../../api/users";
import { type CommandOperation, type PendingCommands, setIfAlive, trySetIfAlive } from "../pending";
import { advanceCommandReload } from "../pending/sources";
import type { AdminActionFeedback } from "../presentation";
import { adminUserGeneratedKeyDelivery } from "./create";

/** 用户行可打开的确认对话框。 */
export type AdminUserDialogKind =
  | "forum_account_id"
  | "reissue_key"
  | "revoke_key"
  | "blacklist"
  | "unblacklist";

/** 对话框标题随操作类型与目标用户名变化。 */
export function adminUserDialogKindTitle(kind: AdminUserDialogKind, username: string): string {
  switch (kind) {
    case "forum_account_id":
      return t("admin-users-forum-account-dialog-title", { username });
    case "reissue_key":
      return t("admin-users-reissue-dialog-title", { username });
    case "revoke_key":
      return t("admin-users-revoke-dialog-title", { username });
    case "blacklist":
      return t("admin-users-blacklist-dialog-title", { username });
    case "unblacklist":
      return t("admin-users-unblacklist-dialog-title", { username });
    default:
      return unreachable(kind);
  }
}

/** 对话框统一的取消/确认动作区；忙碌或阻断时禁止提交。 */
export function AdminUserDialogActions(props: {
  readonly busy: boolean;
  readonly blocked: boolean;
  readonly onCancel: (event: MouseEvent) => void;
  readonly confirmLabel: string;
  readonly confirmClass: string;
}): JSX.Element {
  return (
    <div class="admin-shell__dialog-actions">
      <button type="button" disabled={props.busy} onClick={(event) => props.onCancel(event)}>
        {t("admin-users-action-cancel")}
      </button>
      <button type="submit" class={props.confirmClass} disabled={props.busy || props.blocked}>
        {props.busy ? t("admin-action-working") : props.confirmLabel}
      </button>
    </div>
  );
}

/** 一次性 Key 由会话保留，普通用户反馈仍交付给页面。 */
export function runUserAction(
  owner: PendingCommands,
  entity: string,
  action: string,
  feedback: Cell<AdminActionFeedback | null>,
  reloadGeneration: Cell<bigint>,
  operation: CommandOperation<string | null>,
  feedbackAlive: () => boolean = () => true,
): boolean {
  return owner.submitWithReceipt(
    entity,
    action,
    operation,
    (key, sourceAlive) =>
      adminUserGeneratedKeyDelivery(
        key,
        sourceAlive && trySetIfAlive(feedback, { kind: "success" }, feedbackAlive),
      ),
    (code) => {
      const displayCode = adminApiFailureDisplayCode(code);
      setIfAlive(feedback, { kind: "error", code: displayCode }, feedbackAlive);
      // 成功由 owner 刷新；拒绝或未知也保留原用户页的重读行为。
      advanceCommandReload(reloadGeneration);
    },
  );
}

/** 状态筛选的地址栏取值。 */
export function statusFilterValue(value: AdminUserStatusFilter): string {
  switch (value) {
    case "all":
      return "all";
    case "active":
      return "active";
    case "blacklisted":
      return "blacklisted";
    default:
      return unreachable(value);
  }
}

/** 只认两个明确取值，其余一律回到全部用户。 */
export function parseStatusFilter(value: string): AdminUserStatusFilter {
  switch (value) {
    case "active":
      return "active";
    case "blacklisted":
      return "blacklisted";
    default:
      return "all";
  }
}
