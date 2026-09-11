import { helper } from "./helper";
import type { Base } from "./base";

export class Widget extends BaseWidget implements Base {
  render(): string {
    return helper(this.name);
  }
}

export function main(): void {
  const w: Widget = new Widget();
  w.render();
}
