import { spreadClear, spreadCore, spreadLater } from './spreadcore';

declare function spreadMake(): object;

export const spreadW = { ...spreadCore };
export const assignedW = Object.assign({}, spreadCore);
export const aliasedW = spreadCore;
export const forwardW = { clear: spreadClear };
export const shorthandW = { spreadClear };
export const layeredW = { ...spreadCore, ...spreadLater };
export const overriddenW = { ...spreadCore, info(): void {} };
export const unknownW = { ...spreadCore, ...spreadMake() };
export const cycA = { ...cycB };
export const cycB = { ...cycA };
