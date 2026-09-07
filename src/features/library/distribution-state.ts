/** UI distribution state, independent of Agent execution and link health.
 * The existing Core `desired` flag means a managed global distribution is
 * recorded. Enable/Disable command names remain compatibility identifiers.
 */
export type DistributionState =
  "distributed" | "not_distributed" | "partially_distributed";

export function distributionState(
  desired: readonly boolean[],
): DistributionState {
  if (desired.length > 0 && desired.every(Boolean)) return "distributed";
  return desired.some(Boolean) ? "partially_distributed" : "not_distributed";
}
