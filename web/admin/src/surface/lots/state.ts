// Admin lot 页面使用的短生命周期交互状态。

/** 拍品编辑对话框的两种意图。 */
export type LotDialog = "reprice" | "withdraw";

/** 拍品修改成功后的页面通知。 */
export type LotUpdateNotice = "repriced" | "withdrawn";
