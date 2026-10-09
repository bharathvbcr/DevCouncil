//! C19 2018 layout: geo.rs owns geo/point.rs.
pub mod point;

pub fn geo_entry_c19() -> f64 {
    point::dist_c19(1.0, 2.0) + crate::geo::point::dist_c19(0.0, 0.0)
}
