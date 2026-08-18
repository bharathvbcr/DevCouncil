// TS Audit Blind-spot Syntax Fixture (X1, X2, X3, X20, G18)
import * as utils from "./utils"; // G18: import * as ns

export enum Color {
    Red = "RED",
    Green = "GREEN",
    Blue = "BLUE",
}

export namespace Geometry {
    export interface Point {
        x: number;
        y: number;
    }
}

export declare function externalFn(x: number): void;

export abstract class BaseService {
    abstract execute(): void;
}

export function overloadFn(x: string): string;
export function overloadFn(x: number): number;
export function overloadFn(x: any): any {
    return x;
}

function logDecorator(target: any, propertyKey: string, descriptor: PropertyDescriptor) {
    // X20: decorator as call / annotation
}

export class ServiceConsumer extends BaseService {
    @logDecorator
    execute(): void {
        const item = new utils.Helper(); // X2: new_expression as call
        item.run();
    }
}

export * from "./reexport"; // X3: export * sentinel
