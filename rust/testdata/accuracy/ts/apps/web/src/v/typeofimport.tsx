type ApiModule = typeof import('../services/api');
function innerHelper(): number { return 1; }
export const TypeofProvider = ({ n }: { n: number }) => <div>{innerHelper() + n}</div>;
export function useTypeof(): number { return innerHelper(); }
