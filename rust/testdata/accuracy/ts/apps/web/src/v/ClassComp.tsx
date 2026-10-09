import React from 'react';
export class Legacy extends React.Component<{ n: number }> {
  componentDidMount(): void { this.load(); }
  load(): void { legacyHelper(); }
  render() { return <div>{this.props.n}</div>; }
}
function legacyHelper(): void {}
