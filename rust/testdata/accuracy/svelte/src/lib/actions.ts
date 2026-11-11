export function focusTrap(node: HTMLElement): void {
  node.focus();
}

export function grow(node: HTMLElement): { duration: number } {
  return { duration: node.clientWidth };
}

export function shrink(node: HTMLElement): { duration: number } {
  return { duration: node.clientHeight };
}

export function reorder(node: HTMLElement): { duration: number } {
  return { duration: node.childElementCount };
}

export function slide(node: HTMLElement): { duration: number } {
  return { duration: node.scrollHeight };
}
