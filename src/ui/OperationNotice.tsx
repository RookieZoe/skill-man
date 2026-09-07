import { useLocale } from "../features/locale/LocaleProvider";
import { IndeterminateProgress } from "./IndeterminateProgress";

/** Foreground work has exactly one progress surface, next to its controls. */
export function OperationNotice({
  busy,
  label,
  cancellable = false,
  cancelHint,
}: {
  busy: boolean;
  label?: string;
  cancellable?: boolean;
  cancelHint?: string;
}) {
  const { t } = useLocale();
  if (!busy) return null;
  return (
    <div className="operation-notice">
      <IndeterminateProgress
        label={label ?? t("operation.foreground.working")}
      />
      <p>
        {cancelHint ??
          t(
            cancellable
              ? "operation.foreground.cancel"
              : "operation.foreground.locked",
          )}
      </p>
    </div>
  );
}
