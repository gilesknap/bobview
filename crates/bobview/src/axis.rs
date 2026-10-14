//! Numeric scales shared by plots, meters and the image colour bar: where the
//! ticks go and how their labels read.

/// Tick values for a linear scale over `lo..=hi`, at a "nice" step
/// (1, 2 or 5 times a power of ten) giving at most `max_ticks` ticks.
/// Returns the ticks and the step.
pub fn ticks(lo: f64, hi: f64, max_ticks: usize) -> (Vec<f64>, f64) {
    let (lo, hi) = if lo <= hi { (lo, hi) } else { (hi, lo) };
    let span = hi - lo;
    if !lo.is_finite() || !hi.is_finite() {
        return (Vec::new(), 0.0);
    }
    if span <= 0.0 {
        return (vec![lo], 0.0);
    }
    let step = nice_step(span, max_ticks);
    let first = (lo / step - 1e-9).ceil();
    let values = (0..)
        .map(|i| (first + i as f64) * step)
        .take_while(|v| *v <= hi + step * 1e-9)
        .take(1000)
        .map(|v| if v.abs() < step * 1e-9 { 0.0 } else { v })
        .collect();
    (values, step)
}

/// The smallest 1/2/5 step that splits `span` into at most `max_ticks - 1` gaps.
fn nice_step(span: f64, max_ticks: usize) -> f64 {
    let gaps = (max_ticks.max(2) - 1) as f64;
    let mag = 10f64.powf((span / gaps).log10().floor());
    [1.0, 2.0, 5.0, 10.0]
        .iter()
        .map(|m| m * mag)
        .find(|s| span / s <= gaps + 1e-9)
        .unwrap_or(10.0 * mag)
}

/// Decade ticks for a log scale over `lo..=hi` (both must be positive).
pub fn log_ticks(lo: f64, hi: f64) -> Vec<f64> {
    if lo <= 0.0 || hi <= lo || !hi.is_finite() {
        return Vec::new();
    }
    let (a, b) = (lo.log10().ceil() as i32, hi.log10().floor() as i32);
    (a..=b).take(100).map(|e| 10f64.powi(e)).collect()
}

/// Widen `lo..hi` outwards to whole ticks, as Phoebus does when autoscaling.
/// An empty range is opened up around its value.
pub fn nice_range(lo: f64, hi: f64, max_ticks: usize) -> (f64, f64) {
    if !lo.is_finite() || !hi.is_finite() {
        return (0.0, 100.0);
    }
    if hi <= lo {
        let d = if lo == 0.0 { 1.0 } else { lo.abs() * 0.1 };
        return nice_range(lo - d, lo + d, max_ticks);
    }
    let step = nice_step(hi - lo, max_ticks);
    ((lo / step).floor() * step, (hi / step).ceil() * step)
}

/// A tick label, with as many decimals as the step needs.
pub fn label(v: f64, step: f64) -> String {
    let big = v.abs().max(step.abs());
    if big >= 1e6 || (step > 0.0 && step < 1e-4) {
        let s = format!("{v:.1e}");
        return s.replace(".0e", "e");
    }
    let decimals = if step > 0.0 {
        (-step.log10().floor()).max(0.0) as usize
    } else {
        // A lone tick: just enough to show the value.
        let s = format!("{v}");
        return if s.len() > 8 { format!("{v:.3}") } else { s };
    };
    format!("{v:.decimals$}")
}

/// Maps values to positions along a scale.
#[derive(Clone, Copy, Debug)]
pub struct Scale {
    pub lo: f64,
    pub hi: f64,
    pub log: bool,
    /// Screen positions of `lo` and `hi`.
    pub from: f32,
    pub to: f32,
}

impl Scale {
    fn t(&self, v: f64) -> f64 {
        if self.log && self.lo > 0.0 {
            let l = |x: f64| x.max(f64::MIN_POSITIVE).log10();
            (l(v) - l(self.lo)) / (l(self.hi) - l(self.lo))
        } else {
            (v - self.lo) / (self.hi - self.lo)
        }
    }

    /// Screen position of `v`; values outside the range land outside `from..to`.
    pub fn pos(&self, v: f64) -> f32 {
        let t = self.t(v);
        let t = if t.is_finite() { t } else { 0.0 };
        self.from + (self.to - self.from) * t as f32
    }

    /// Ticks for a scale `length` pixels long with labels about `label_len` apart.
    pub fn ticks(&self, label_len: f32) -> (Vec<f64>, f64) {
        if self.log && self.lo > 0.0 {
            let t = log_ticks(self.lo, self.hi);
            if t.len() >= 2 {
                return (t, 0.0);
            }
        }
        let length = (self.to - self.from).abs();
        let n = (length / label_len.max(1.0)).floor().max(2.0) as usize;
        ticks(self.lo, self.hi, n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticks_use_nice_steps() {
        let (t, step) = ticks(-25.0, 25.0, 6);
        assert_eq!(step, 10.0);
        assert_eq!(t, vec![-20.0, -10.0, 0.0, 10.0, 20.0]);
        let (t, step) = ticks(-360.0, 360.0, 10);
        assert_eq!(step, 100.0);
        assert_eq!(t.first(), Some(&-300.0));
        assert_eq!(t.last(), Some(&300.0));
        let (t, step) = ticks(0.0, 1.0, 6);
        assert_eq!(step, 0.2);
        assert_eq!(t.len(), 6);
        assert_eq!(ticks(0.0, 100.0, 11).0.len(), 11);
    }

    #[test]
    fn ticks_handle_degenerate_ranges() {
        assert_eq!(ticks(5.0, 5.0, 5), (vec![5.0], 0.0));
        assert!(ticks(f64::NAN, 1.0, 5).0.is_empty());
        // Reversed ranges are fine.
        assert_eq!(ticks(10.0, 0.0, 3).0, vec![0.0, 5.0, 10.0]);
    }

    #[test]
    fn labels_match_the_step() {
        assert_eq!(label(20.0, 10.0), "20");
        assert_eq!(label(-0.4, 0.2), "-0.4");
        assert_eq!(label(0.25, 0.05), "0.25");
        assert_eq!(label(2e6, 1e6), "2e6");
    }

    #[test]
    fn nice_range_rounds_outwards() {
        assert_eq!(nice_range(0.0, 1023.0, 8), (0.0, 1200.0));
        assert_eq!(nice_range(3.2, 9.7, 8), (3.0, 10.0));
        let (lo, hi) = nice_range(5.0, 5.0, 8);
        assert!(lo < 5.0 && hi > 5.0);
    }

    #[test]
    fn scale_maps_linear_and_log() {
        let s = Scale {
            lo: -25.0,
            hi: 25.0,
            log: false,
            from: 0.0,
            to: 100.0,
        };
        assert_eq!(s.pos(0.0), 50.0);
        assert_eq!(s.pos(25.0), 100.0);
        let l = Scale {
            lo: 1.0,
            hi: 1000.0,
            log: true,
            from: 100.0,
            to: 0.0,
        };
        assert!((l.pos(10.0) - 66.666_67).abs() < 1e-3);
        assert_eq!(l.ticks(10.0).0, vec![1.0, 10.0, 100.0, 1000.0]);
    }
}
