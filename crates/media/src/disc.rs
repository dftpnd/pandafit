use crate::probe::ProbeError;
use std::process::Command;

#[derive(Debug, Clone, PartialEq)]
pub struct DiscStatus {
    pub device: String,
    pub capacity_bytes: u64,
    pub blank: bool,
    pub media_name: String,
}

pub trait DiscDevice {
    fn status(&self, device: &str) -> Result<DiscStatus, ProbeError>;
}

pub fn parse_mediainfo(text: &str, device: &str) -> Option<DiscStatus> {
    let mut capacity = None;
    let mut blank = false;
    let mut media_name = String::new();

    for line in text.lines() {
        let l = line.trim();
        if let Some(rest) = l.strip_prefix("Mounted Media:") {
            media_name = rest.trim().to_string();
        }
        if let Some(rest) = l.strip_prefix("Disc status:") {
            blank = rest.trim() == "blank";
        }
        if let Some(eq) = l.rfind('=') {
            if l.contains('*') && (l.contains("unformatted") || l.contains("formatted")) {
                if let Ok(v) = l[eq + 1..].trim().parse::<u64>() {
                    capacity = Some(capacity.map_or(v, |c: u64| c.max(v)));
                }
            }
        }
    }

    let capacity_bytes = capacity?;
    if media_name.is_empty() {
        return None;
    }
    Some(DiscStatus { device: device.to_string(), capacity_bytes, blank, media_name })
}

pub struct SgDiscDevice;

impl DiscDevice for SgDiscDevice {
    fn status(&self, device: &str) -> Result<DiscStatus, ProbeError> {
        let out = Command::new("dvd+rw-mediainfo").arg(device).output()?;
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        parse_mediainfo(&text, device)
            .ok_or_else(|| ProbeError::Failed(format!("в приводе {device} нет читаемого диска")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../tests/fixtures/mediainfo.bdr25.txt");

    #[test]
    fn capacity_comes_from_the_sector_count_not_from_a_nominal_table() {
        let s = parse_mediainfo(FIXTURE, "/dev/sr0").unwrap();
        assert_eq!(s.capacity_bytes, 25_025_314_816);
    }

    #[test]
    fn blank_disc_is_recognised() {
        let s = parse_mediainfo(FIXTURE, "/dev/sr0").unwrap();
        assert!(s.blank);
        assert!(s.media_name.contains("BD-R"));
    }

    #[test]
    fn non_blank_disc_is_reported_as_such() {
        let text = "Mounted Media:         41h, BD-R SRM\n\
                    Disc status:           complete\n\
                    READ FORMAT CAPACITIES:\n unformatted:\t12219392*2048=25025314816\n";
        let s = parse_mediainfo(text, "/dev/sr0").unwrap();
        assert!(!s.blank);
    }

    #[test]
    fn empty_tray_yields_nothing() {
        assert!(parse_mediainfo("no medium present", "/dev/sr0").is_none());
    }
}
