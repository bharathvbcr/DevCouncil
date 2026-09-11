import { helper } from "./helper";

export class Widget extends BaseWidget {
  render(): string {
    return helper("x");
  }
}
