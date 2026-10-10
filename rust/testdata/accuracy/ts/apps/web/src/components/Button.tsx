import { forwardRef, memo } from 'react';
export function Button({ onPress, label }: { onPress: () => void; label: string }) {
  return <button onClick={onPress}>{label}</button>;
}
export const MemoBadge = memo(function MemoBadge({ n }: { n: number }) { return <span>{badgeText(n)}</span>; });
function badgeText(n: number): string { return String(n); }
export const FancyInput = forwardRef<HTMLInputElement, { v: string }>((props, ref) => {
  return <input ref={ref} value={fancyValue(props.v)} />;
});
function fancyValue(v: string): string { return v; }
export const Parts = { Header: function PartsHeader() { return <h1>{headerText()}</h1>; } };
function headerText(): string { return 'h'; }
