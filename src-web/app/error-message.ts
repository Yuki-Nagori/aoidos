import { billingErrorDescriptor } from "../features/billing/utils/billing-error";
import type { BillingFailure } from "../features/billing/types";

type ErrorDescriptor = { key: string } | { key: string; values: { code: string } };

/** 将应用故障转换为稳定的本地化文案，不展示后端自由文本。 */
export function appErrorDescriptor(failure: BillingFailure, reason?: string): ErrorDescriptor {
  if (
    failure.code === "app.bad-request" &&
    (reason === "stalePresenceRevision" || reason === "targetUnavailable")
  ) {
    return { key: "game.presenceChanged" };
  }
  if (failure.code === "app.event-failed") {
    return { key: "errors.appEventFailed" };
  }
  return billingErrorDescriptor(failure);
}
