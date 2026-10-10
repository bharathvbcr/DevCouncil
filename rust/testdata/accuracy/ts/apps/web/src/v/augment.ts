// V19 (added for the dead-policy card): module augmentation and `declare global`.
// Every declaration below augments a type that lives elsewhere; none of them is
// a dead-code candidate, whatever the call graph says.
export {};

declare global {
  interface Window {
    fxFlag?: boolean;
  }
}

declare module './logger' {
  interface LoggerExtras {
    tag: string;
  }
}
