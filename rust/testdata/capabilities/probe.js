import { helper } from "./helper";

export class Widget extends BaseWidget {
  render() {
    return helper(this.name);
  }
}

export function main() {
  const w = new Widget();
  w.render();
}
