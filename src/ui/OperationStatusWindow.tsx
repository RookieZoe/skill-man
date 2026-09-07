import { useEffect, useState } from "react";

import { IndeterminateProgress } from "./IndeterminateProgress";

const OPERATION_STATUS_DELAY_MS = 250;

export interface OperationStatus {
  id: string;
  title: string;
  detail: string;
  state?: "running" | "completed" | "failed" | "cancelled" | "partial";
}

interface OperationStatusWindowProps {
  ariaLabel: string;
  heading: string;
  operations: readonly OperationStatus[];
  onDismiss?: (id: string) => void;
  dismissLabel?: string;
}

/**
 * A modeless activity surface for native work without a trustworthy numeric
 * total. It stays outside transactional sheets so it never creates a nested
 * dialog or blocks a user from reading the operation that is in progress.
 */
export function OperationStatusWindow({
  ariaLabel,
  heading,
  operations,
  onDismiss,
  dismissLabel,
}: OperationStatusWindowProps) {
  const [isVisible, setIsVisible] = useState(false);

  useEffect(() => {
    const timeout = window.setTimeout(
      () => setIsVisible(true),
      OPERATION_STATUS_DELAY_MS,
    );
    return () => window.clearTimeout(timeout);
  }, []);

  if (operations.length === 0 || !isVisible) return null;

  return (
    <section
      className="operation-status-window"
      role="region"
      aria-label={ariaLabel}
      aria-live="polite"
      aria-atomic="true"
    >
      <header className="operation-status-window-heading">
        <p>{heading}</p>
        <span aria-hidden="true" />
      </header>
      <ul className="operation-status-window-list">
        {operations.map((operation) => (
          <li
            className="operation-status-window-item"
            key={operation.id}
            data-state={operation.state ?? "running"}
          >
            <div className="operation-status-window-item-heading">
              <span aria-hidden="true" />
              <h2>{operation.title}</h2>
            </div>
            <p>{operation.detail}</p>
            {!operation.state || operation.state === "running" ? (
              <IndeterminateProgress label={operation.title} />
            ) : onDismiss ? (
              <button
                type="button"
                className="toolbar-button operation-dismiss"
                onClick={() => onDismiss(operation.id)}
              >
                {dismissLabel}
              </button>
            ) : null}
          </li>
        ))}
      </ul>
    </section>
  );
}
