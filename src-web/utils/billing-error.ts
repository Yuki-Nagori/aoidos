import type { BillingFailure } from "../composables/useBilling";

type BillingErrorDescriptor =
  | {
      key:
        | "errors.appBusy"
        | "errors.appNotFound"
        | "errors.appNotReady"
        | "errors.badRequest"
        | "errors.storeCorrupt"
        | "errors.storeIo"
        | "billing.errors.conflict"
        | "billing.errors.exceeded"
        | "billing.errors.operationFailed"
        | "billing.errors.priceHistoryLimit"
        | "billing.errors.priceMissing"
        | "billing.errors.runLimitMissing";
    }
  | { key: "errors.unknownWithCode"; values: { code: string } };

/** Maps stable billing fault codes to localized messages without exposing backend prose. */
export function billingErrorDescriptor(failure: BillingFailure): BillingErrorDescriptor {
  switch (failure.code) {
    case "app.not-found":
      return { key: "errors.appNotFound" };
    case "app.not-ready":
      return { key: "errors.appNotReady" };
    case "app.bad-request":
      return { key: "errors.badRequest" };
    case "app.busy":
      return { key: "errors.appBusy" };
    case "store.corrupt":
      return { key: "errors.storeCorrupt" };
    case "store.database":
    case "store.io":
      return { key: "errors.storeIo" };
    case "budget.conflict":
      return { key: "billing.errors.conflict" };
    case "budget.exceeded":
      return { key: "billing.errors.exceeded" };
    case "budget.price-history-limit":
      return { key: "billing.errors.priceHistoryLimit" };
    case "budget.price-missing":
      return { key: "billing.errors.priceMissing" };
    case "budget.run-limit-missing":
      return { key: "billing.errors.runLimitMissing" };
    default:
      return failure.code
        ? { key: "errors.unknownWithCode", values: { code: failure.code } }
        : { key: "billing.errors.operationFailed" };
  }
}
