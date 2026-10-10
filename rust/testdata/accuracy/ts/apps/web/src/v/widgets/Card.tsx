import { memo } from 'react';
function Card({ t }: { t: string }) { return <div>{cardText(t)}</div>; }
function cardText(t: string): string { return t; }
export default memo(Card);
