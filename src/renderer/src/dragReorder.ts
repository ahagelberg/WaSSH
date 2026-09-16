/**
 * Convert a visual gap index to the insert index expected by a reorder handler
 * (index in the array after the dragged item is removed).
 * Returns null when the drop would not change order.
 */
export function insertIndexFromGap(from: number, gap: number): number | null {
  if (gap === from || gap === from + 1) {
    return null
  }
  return gap > from ? gap - 1 : gap
}
