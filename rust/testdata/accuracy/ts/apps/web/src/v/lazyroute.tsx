export function Component() { return <p>{lazyText()}</p>; }
function lazyText(): string { return 'l'; }
