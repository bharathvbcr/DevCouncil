export type Status = "todo" | "doing" | "done";

export interface Card {
  id: string;
  priority: number;
  status: Status;
}

export function columnLabel(status: Status): string {
  return status.toUpperCase();
}

export function priorityLabel(card: Card): string {
  return `P${card.priority}`;
}

export function cardTone(card: Card): string {
  return card.status === "done" ? "muted" : "live";
}

export function padCell(index: number): string {
  return String(index).padStart(3, "0");
}
