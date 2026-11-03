// Retention policy editing. A policy is
// { enabled, days, dictations, calls, notes }, matching the controller.

export const defaultPolicy = Object.freeze({
  enabled: false,
  days: 90,
  dictations: true,
  calls: true,
  notes: true,
});

export const MAX_RETENTION_DAYS = 36500;

export function samePolicy(left, right) {
  return ["enabled", "days", "dictations", "calls", "notes"].every(
    (key) => (left?.[key] ?? null) === (right?.[key] ?? null),
  );
}

export function policyProblem(policy) {
  const days = Number(policy?.days);
  if (!Number.isInteger(days) || days < 1 || days > MAX_RETENTION_DAYS)
    return `Choose a period between 1 and ${MAX_RETENTION_DAYS} days.`;
  if (policy.enabled && !policy.dictations && !policy.calls && !policy.notes)
    return "Choose at least one kind of item.";
  return null;
}

// What saving the draft requires: nothing, a direct save, or a preview and
// explicit confirmation first (any enabled policy can delete items).
export function retentionStep(saved, draft, preview) {
  if (samePolicy(saved, draft)) return "saved";
  if (policyProblem(draft)) return "invalid";
  if (!draft.enabled) return "save";
  if (preview && !preview.error && samePolicy(preview.policy, draft))
    return "confirm";
  return "preview";
}
