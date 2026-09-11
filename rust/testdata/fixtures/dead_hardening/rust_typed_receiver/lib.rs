pub struct Engine;
impl Engine {
    pub fn tick(&self) {}
    pub fn new() -> Self {
        Self
    }
}

pub struct Holder {
    engine: Engine,
}

pub fn via_param(engine: Engine) {
    engine.tick();
}

pub fn via_field(h: Holder) {
    h.engine.tick();
}

pub fn via_local() {
    let engine = Engine::new();
    engine.tick();
}
