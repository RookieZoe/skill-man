interface IndeterminateProgressProps {
  label: string;
  className?: string;
}

/**
 * Honest busy feedback for native work that does not expose a stable numeric
 * completion count. Deliberately omits aria-valuenow: this is activity, not
 * fabricated percentage progress.
 */
export function IndeterminateProgress({
  label,
  className,
}: IndeterminateProgressProps) {
  return (
    <div
      className={["operation-progress", className].filter(Boolean).join(" ")}
      aria-busy="true"
    >
      <span className="operation-progress-label">{label}</span>
      <div
        className="operation-progress-track"
        role="progressbar"
        aria-label={label}
        aria-valuetext={label}
      >
        <span className="operation-progress-indicator" />
      </div>
    </div>
  );
}
