// A same-named neighbour: the component imports its labels from format.ts, so
// no edge of any tier may reach these.
export function columnLabel(status: string): string {
  return status;
}

export function priorityLabel(card: { priority: number }): string {
  return String(card.priority);
}
