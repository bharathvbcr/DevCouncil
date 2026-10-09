//! C26 operator traits and implicit trait calls (Add, Index, Drop, From/into, ? conversion).

use std::ops::{Add, Index};

#[derive(Clone, Copy)]
pub struct V(pub i32);

impl Add for V {
    type Output = V;
    fn add(self, o: V) -> V {
        V(self.0 + o.0)
    }
}

pub struct Table(pub Vec<u8>);
impl Index<usize> for Table {
    type Output = u8;
    fn index(&self, i: usize) -> &u8 {
        &self.0[i]
    }
}

pub struct Res;
impl Drop for Res {
    fn drop(&mut self) {}
}

pub struct MyErr;
impl From<std::num::ParseIntError> for MyErr {
    fn from(_: std::num::ParseIntError) -> Self {
        MyErr
    }
}
pub struct Meters(pub u32);
impl From<u32> for Meters {
    fn from(x: u32) -> Self {
        Meters(x)
    }
}

pub fn c26_caller(t: &Table) -> Result<i32, MyErr> {
    let s = V(1) + V(2); // V::add
    let x = t[0]; // Table::index
    let _r = Res; // Res::drop at scope end
    let m: Meters = 5u32.into(); // Meters::from
    let n: i32 = "7".parse::<i32>()?; // MyErr::from via ?
    Ok(s.0 + x as i32 + m.0 as i32 + n)
}
