/// f32 wrapper that displays minimal decimal places.
#[derive(Debug, Clone, Copy)]
pub struct TerseF32(pub f32);

impl std::fmt::Display for TerseF32 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if pseudo_int(self.0.into()) {
            write!(f, "{:.0}", self.0)
        } else if pseudo_int(f64::from(self.0) * 10.0) {
            write!(f, "{:.1}", self.0)
        } else if pseudo_int(f64::from(self.0) * 100.0) {
            write!(f, "{:.2}", self.0)
        } else {
            self.0.fmt(f)
        }
    }
}

#[inline]
fn pseudo_int(f: f64) -> bool {
    !(0.0002..=0.9998).contains(&f.fract())
}
