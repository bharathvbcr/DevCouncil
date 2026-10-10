import { logger } from './logger';
export function useLogger(): number { logger.clear(); logger.log('x'); return logger.getLogs().length; }
