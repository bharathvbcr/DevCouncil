type ApiShape = import('../services/api').Shape;
function importTypeHelper(): number { return 1; }
export const ImportTypeProvider = ({ n }: { n: number }) => <div>{importTypeHelper() + n}</div>;
export function useImportType(): number { return importTypeHelper(); }
