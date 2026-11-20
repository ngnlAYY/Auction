// 凭据删除确认对话框。

import { Dialog } from "@auction/web-shared/ui/components/dialog";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";

import type { AdminCredential } from "../../api/credentials";

/** 待删除凭据、删除中的忙碌/阻断标记与确认、取消回调。 */
export interface CredentialDeleteDialogProps {
  readonly credential: AdminCredential;
  readonly busy: boolean;
  readonly blocked: boolean;
  readonly onConfirm: (credential: AdminCredential) => void;
  readonly onCancel: () => void;
}

/** 删除前的强制确认；取消按钮自动聚焦，忙碌时两个按钮都不可用。 */
export function CredentialDeleteDialog(props: CredentialDeleteDialogProps): JSX.Element {
  return (
    // 外壳（遮罩/标题栏/关闭守卫）由共享 Dialog 接管。
    <Dialog
      title={t("admin-credentials-delete-title")}
      closeLabel={t("admin-credentials-cancel")}
      busy={props.busy}
      onClose={() => props.onCancel()}
    >
      <p>{t("admin-credentials-delete-warning", { label: props.credential.label })}</p>
      <div class="admin-shell__dialog-actions">
        <button
          type="button"
          class="admin-btn-secondary"
          disabled={props.busy}
          autofocus={true}
          onClick={() => props.onCancel()}
        >
          {t("admin-credentials-cancel")}
        </button>
        <button
          type="button"
          class="admin-btn-danger"
          disabled={props.busy || props.blocked}
          onClick={() => props.onConfirm(props.credential)}
        >
          {props.busy ? t("admin-action-working") : t("admin-credentials-action-delete")}
        </button>
      </div>
    </Dialog>
  );
}
