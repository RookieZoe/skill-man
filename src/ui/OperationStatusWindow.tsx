import { useEffect, useState } from "react";

import { IndeterminateProgress } from "./IndeterminateProgress";

const OPERATION_STATUS_DELAY_MS = 250;

export interface OperationStatus {
  id: string;
  title: string;
  detail: string;
}

interface OperationStatusWindowProps {
  ariaLabel: string;
  heading: string;
  operations: readonly OperationStatus[];
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
          <li className="operation-status-window-item" key={operation.id}>
            <div className="operation-status-window-item-heading">
              <span aria-hidden="true" />
              <h2>{operation.title}</h2>
            </div>
            <p>{operation.detail}</p>
            <IndeterminateProgress label={operation.title} />
          </li>
        ))}
      </ul>
    </section>
  );
}
