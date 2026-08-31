#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Confidence {
    Exact,
    Sampled,
    Guessed,
}

impl Confidence {
    /// Относительный запас на верхнюю границу интервала.
    pub fn margin(self) -> f64 {
        match self {
            Confidence::Exact => 0.0,
            Confidence::Sampled => 0.05,
            Confidence::Guessed => 0.20,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Estimated<T> {
    pub value: T,
    pub confidence: Confidence,
}

impl Estimated<u64> {
    pub fn exact(value: u64) -> Self {
        Self { value, confidence: Confidence::Exact }
    }
    pub fn sampled(value: u64) -> Self {
        Self { value, confidence: Confidence::Sampled }
    }
    pub fn guessed(value: u64) -> Self {
        Self { value, confidence: Confidence::Guessed }
    }

    /// Верхняя граница интервала — то, по чему принимается решение «влезет».
    pub fn upper(self) -> u64 {
        self.value + (self.value as f64 * self.confidence.margin()).round() as u64
    }

    pub fn sum(items: impl IntoIterator<Item = Estimated<u64>>) -> Estimated<u64> {
        let mut value = 0u64;
        let mut confidence = Confidence::Exact;
        for it in items {
            value += it.value;
            confidence = confidence.max(it.confidence);
        }
        Estimated { value, confidence }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upper_bound_grows_with_uncertainty() {
        assert_eq!(Estimated::exact(1000).upper(), 1000);
        assert_eq!(Estimated::sampled(1000).upper(), 1050);
        assert_eq!(Estimated::guessed(1000).upper(), 1200);
    }

    #[test]
    fn sum_takes_worst_confidence() {
        let s = Estimated::sum([Estimated::exact(10), Estimated::guessed(5)]);
        assert_eq!(s.value, 15);
        assert_eq!(s.confidence, Confidence::Guessed);
    }
}
