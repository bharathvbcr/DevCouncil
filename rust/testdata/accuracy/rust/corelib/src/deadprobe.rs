//! C41 dead-code probes: private items whose only use is one of the gap shapes.
//! Expected dead: truly_dead_c99 only. Everything else is live.

fn truly_dead_c99() -> u8 {
    99
}

// Only use: inside a macro_rules! body invoked below.
fn via_macro_body_c41() -> u8 {
    41
}
macro_rules! call_hidden_c41 {
    () => {
        via_macro_body_c41()
    };
}

struct Hidden;
impl Hidden {
    // Only use: through Deref from Wrap.
    fn via_deref_c41(&self) -> u8 {
        1
    }
    // Only use: on a value bound by `?`.
    fn via_question_c41(&self) -> u8 {
        2
    }
    // Only use: inside a closure over a slice element.
    fn via_closure_c41(&self) -> u8 {
        3
    }
}
struct Wrap(Hidden);
impl std::ops::Deref for Wrap {
    type Target = Hidden;
    fn deref(&self) -> &Hidden {
        &self.0
    }
}

trait Act {
    fn act_c41(&self) -> u8;
}
struct Impl1;
impl Act for Impl1 {
    // Only use: dyn dispatch.
    fn act_c41(&self) -> u8 {
        4
    }
}

fn make_hidden_c41() -> Result<Hidden, ()> {
    Ok(Hidden)
}

// Inline-module twins with one name: two different functions.
mod alpha {
    pub fn twin_c42() -> u8 {
        1
    }
}
mod beta {
    #[allow(dead_code)]
    pub fn twin_c42() -> u8 {
        2
    }
}

pub fn deadprobe_entry() -> Result<u8, ()> {
    let a = call_hidden_c41!();
    let w = Wrap(Hidden);
    let b = w.via_deref_c41();
    let h = make_hidden_c41()?;
    let c = h.via_question_c41();
    let v = vec![Hidden];
    let d: u8 = v.iter().map(|x| x.via_closure_c41()).sum();
    let o: Box<dyn Act> = Box::new(Impl1);
    let e = o.act_c41();
    let f = alpha::twin_c42(); // beta::twin_c42 is truly dead
    Ok(a + b + c + d + e + f)
}
