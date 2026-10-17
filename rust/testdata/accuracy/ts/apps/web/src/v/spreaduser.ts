import {
  aliasedW,
  assignedW,
  cycA,
  forwardW,
  layeredW,
  overriddenW,
  shorthandW,
  spreadW,
  unknownW,
} from './spreadbuild';

export function useSpread(): void { spreadW.info(); }
export function useAssign(): void { assignedW.info(); }
export function useAlias(): void { aliasedW.info(); }
export function useForward(): void { forwardW.clear(); }
export function useShorthand(): void { shorthandW.spreadClear(); }
export function useLayered(): void { layeredW.info(); }
export function useOverridden(): void { overriddenW.info(); }
export function useUnknown(): void { unknownW.info(); }
export function useCycle(): void { cycA.info(); }
