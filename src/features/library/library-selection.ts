export interface SelectionModifiers {
  shiftKey: boolean;
  metaKey: boolean;
  ctrlKey: boolean;
}

/** IDs and order come from the visible, expanded Library rows, never names. */
export function selectLibrarySkill(
  selected: string[],
  anchor: string | null,
  clicked: string,
  visibleIds: string[],
  modifiers: SelectionModifiers,
) {
  const target = visibleIds.indexOf(clicked);
  if (target < 0) return { ids: selected, anchor };
  if (modifiers.shiftKey) {
    const startId = selected.length
      ? anchor && selected.includes(anchor) && visibleIds.includes(anchor)
        ? anchor
        : visibleIds.find((id) => selected.includes(id))
      : undefined;
    const start = startId ? visibleIds.indexOf(startId) : 0;
    return {
      ids: visibleIds.slice(
        Math.min(start, target),
        Math.max(start, target) + 1,
      ),
      anchor: visibleIds[start],
    };
  }
  if (modifiers.metaKey || modifiers.ctrlKey) {
    const ids = selected.includes(clicked)
      ? selected.filter((id) => id !== clicked)
      : [...selected, clicked];
    return {
      ids,
      anchor: ids.includes(clicked) ? clicked : (ids.at(-1) ?? null),
    };
  }
  return { ids: [clicked], anchor: clicked };
}
