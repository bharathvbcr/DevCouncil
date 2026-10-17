export class FlEngine {
  run(): number {
    return 1;
  }
}

export class FlOther {
  run(): number {
    return 2;
  }
}

export class FlHolderA {
  engine: FlEngine = new FlEngine();

  go(): number {
    return this.engine.run();
  }
}

export class FlHolderB {
  engine;

  constructor() {
    this.engine = new FlOther();
  }

  go(): number {
    return this.engine.run();
  }
}
