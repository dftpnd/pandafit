#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tick {
    pub position_s: f64,
    pub bytes_written: u64,
    pub speed: Option<f64>,
}

#[derive(Debug, Default)]
pub struct FfmpegProgressState {
    position_s: f64,
    bytes_written: u64,
    speed: Option<f64>,
}

pub fn parse_ffmpeg_progress(st: &mut FfmpegProgressState, line: &str) -> Option<Tick> {
    let (key, value) = line.trim().split_once('=')?;
    match key {
        "out_time_us" => {
            st.position_s = value.parse::<f64>().ok()? / 1_000_000.0;
            None
        }
        "total_size" => {
            st.bytes_written = value.parse().ok()?;
            None
        }
        "speed" => {
            st.speed = value.trim_end_matches('x').parse().ok();
            None
        }
        "progress" => Some(Tick {
            position_s: st.position_s,
            bytes_written: st.bytes_written,
            speed: st.speed,
        }),
        _ => None,
    }
}

pub fn parse_growisofs_line(line: &str) -> Option<f64> {
    let idx = line.find('%')?;
    let head = &line[..idx];
    let num: String = head
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    num.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ffmpeg_progress_block_yields_a_tick_on_the_terminator() {
        let mut st = FfmpegProgressState::default();
        assert!(parse_ffmpeg_progress(&mut st, "out_time_us=125000000").is_none());
        assert!(parse_ffmpeg_progress(&mut st, "total_size=1048576").is_none());
        assert!(parse_ffmpeg_progress(&mut st, "speed=2.5x").is_none());
        let tick = parse_ffmpeg_progress(&mut st, "progress=continue").expect("нет тика");
        assert_eq!(tick.position_s, 125.0);
        assert_eq!(tick.bytes_written, 1_048_576);
        assert_eq!(tick.speed, Some(2.5));
    }

    #[test]
    fn ffmpeg_end_marker_also_yields_a_tick() {
        let mut st = FfmpegProgressState::default();
        parse_ffmpeg_progress(&mut st, "out_time_us=200000000");
        assert!(parse_ffmpeg_progress(&mut st, "progress=end").is_some());
    }

    #[test]
    fn garbage_lines_are_ignored() {
        let mut st = FfmpegProgressState::default();
        assert!(parse_ffmpeg_progress(&mut st, "").is_none());
        assert!(parse_ffmpeg_progress(&mut st, "какой-то мусор").is_none());
        assert!(parse_ffmpeg_progress(&mut st, "out_time_us=не_число").is_none());
    }

    #[test]
    fn growisofs_percentage_is_recognised() {
        assert_eq!(parse_growisofs_line(" 12.3% done, estimate finish Sun Aug 31"), Some(12.3));
        assert_eq!(parse_growisofs_line("Executing 'builtin_dd'"), None);
    }
}
