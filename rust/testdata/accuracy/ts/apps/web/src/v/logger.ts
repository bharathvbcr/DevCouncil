function getLogs(): string[] { return []; }
const core = { log(msg: string): void { void msg; } };
export const logger = { ...core, getLogs, clear: () => clearLogs() };
function clearLogs(): void {}
