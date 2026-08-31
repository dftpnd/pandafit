/// Замер среднего битрейта в момент времени.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    pub t_s: f64,
    pub bps: u64,
}

/// Кривая «битрейт по времени», построенная выборочным сканированием файла.
///
/// Нужна ровно для одного: честно считать, сколько байт экономит обрезка.
/// Титры темнее и статичнее среднего кадра, поэтому пропорциональная оценка
/// («отрезали 7% длительности — минус 7% размера») завышает экономию в разы.
#[derive(Debug, Clone, PartialEq)]
pub struct BitrateProfile {
    pub track_index: usize,
    samples: Vec<Sample>,
}

impl BitrateProfile {
    pub fn from_samples(track_index: usize, mut samples: Vec<Sample>) -> Self {
        samples.sort_by(|a, b| a.t_s.partial_cmp(&b.t_s).unwrap());
        Self { track_index, samples }
    }

    pub fn samples(&self) -> &[Sample] {
        &self.samples
    }

    pub fn is_empty(&self) -> bool {
        self.samples.len() < 2
    }

    /// Интеграл под кривой методом трапеций, в байтах.
    pub fn bytes_between(&self, start_s: f64, end_s: f64) -> u64 {
        if self.is_empty() || end_s <= start_s {
            return 0;
        }
        let mut bits = 0.0f64;
        for pair in self.samples.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            let seg_start = a.t_s.max(start_s);
            let seg_end = b.t_s.min(end_s);
            if seg_end <= seg_start {
                continue;
            }
            let span = b.t_s - a.t_s;
            if span <= 0.0 {
                continue;
            }
            // Линейная интерполяция битрейта на границах перекрытия.
            let at = |t: f64| {
                let k = (t - a.t_s) / span;
                a.bps as f64 + (b.bps as f64 - a.bps as f64) * k
            };
            let avg = (at(seg_start) + at(seg_end)) / 2.0;
            bits += avg * (seg_end - seg_start);
        }
        (bits / 8.0).round() as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Кривая: первые 100 с идут на 10 Мбит/с, дальше 100 с титров на 1 Мбит/с.
    fn film_with_credits() -> BitrateProfile {
        BitrateProfile::from_samples(
            0,
            vec![
                Sample { t_s: 0.0, bps: 10_000_000 },
                Sample { t_s: 100.0, bps: 10_000_000 },
                Sample { t_s: 100.001, bps: 1_000_000 },
                Sample { t_s: 200.0, bps: 1_000_000 },
            ],
        )
    }

    #[test]
    fn integrates_bytes_over_a_range() {
        let p = film_with_credits();
        // 100 с по 10 Мбит/с = 125 МБ
        assert_eq!(p.bytes_between(0.0, 100.0), 125_000_000);
    }

    #[test]
    fn cutting_credits_saves_much_less_than_proportional_guess() {
        let p = film_with_credits();
        let full = p.bytes_between(0.0, 200.0);
        let trimmed = p.bytes_between(0.0, 100.0);
        let saved = full - trimmed;
        // Наивная пропорция обещала бы половину; на деле титры весят вдесятеро меньше.
        assert!(saved * 5 < full, "экономия {saved} из {full} должна быть много меньше половины");
    }

    #[test]
    fn range_outside_samples_is_clamped() {
        let p = film_with_credits();
        assert_eq!(p.bytes_between(-50.0, 0.0), 0);
        assert_eq!(p.bytes_between(500.0, 600.0), 0);
    }
}
