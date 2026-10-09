import { exportedArrow, Mode, modeName } from './arrows.js';
import { heavyCompute } from '../pages/heavy.ts';
export async function useArrow(): Promise<string> { await exportedArrow(1); heavyCompute(); return modeName(Mode.A); }
