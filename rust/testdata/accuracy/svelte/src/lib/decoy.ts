// A same-named neighbour: the component imports its labels from format.ts, so
// no edge of any tier may reach these.
export function columnLabel(status: string): string {
  return status;
}

export function priorityLabel(card: { priority: number }): string {
  return String(card.priority);
}

// svelte/transition's name, local to nothing: a directive that names the imported one
// must not reach this.
export function fade(): void {}
