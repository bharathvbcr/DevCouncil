import { helper } from "./helper";

export class Panel extends BasePanel {
  draw(): JSX.Element {
    return <Inner value={helper(1)} />;
  }
}
