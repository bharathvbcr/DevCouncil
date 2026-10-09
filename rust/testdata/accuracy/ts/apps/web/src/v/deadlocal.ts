function neverCalledLocal(): number { return 1; }
const neverCalledArrow = (): number => 2;
export function liveEntry(): number { return 3; }
