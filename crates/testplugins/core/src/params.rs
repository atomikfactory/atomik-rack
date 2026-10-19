//! A bank of plain params for the cap and rescan tests (`atomik.test.params`).

/// 2 000 params with ids 0 to 1 999, values 0 to 1, and one more (id 2 000) that appears when
/// [`ParamBank::add_extra`] is called, to test the host's param-table rescan.
#[derive(Debug, Clone)]
pub struct ParamBank {
    values: Vec<f64>,
}

impl ParamBank {
    /// The id of the param that appears at run time.
    pub const EXTRA_ID: u32 = 2_000;
    const BASE: usize = 2_000;

    /// A bank of 2 000 zeros.
    pub fn new() -> Self {
        let mut values = Vec::with_capacity(Self::BASE + 1);
        values.resize(Self::BASE, 0.0);
        Self { values }
    }

    /// Number of params now.
    pub fn count(&self) -> usize {
        self.values.len()
    }

    /// Whether the extra param exists.
    pub fn extra_present(&self) -> bool {
        self.values.len() > Self::BASE
    }

    /// Makes the extra param appear.
    pub fn add_extra(&mut self) {
        if !self.extra_present() {
            self.values.push(0.0);
        }
    }

    /// Sets a value, clamped to 0 to 1 (NaN and unknown ids are ignored).
    pub fn set(&mut self, id: u32, value: f64) {
        if let Some(v) = self.values.get_mut(id as usize)
            && !value.is_nan()
        {
            *v = value.clamp(0.0, 1.0);
        }
    }

    /// A value (0 for an unknown id).
    pub fn get(&self, id: u32) -> f64 {
        self.values.get(id as usize).copied().unwrap_or(0.0)
    }
}

impl Default for ParamBank {
    fn default() -> Self {
        Self::new()
    }
}
