import { ImportTypeProvider, useImportType } from './importtype';
export function ImportTypeUser() { const x = useImportType(); return <ImportTypeProvider n={x} />; }
