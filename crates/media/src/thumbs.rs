use crate::probe::ProbeError;
use pandafit_core::MediaInfo;
use std::path::{Path, PathBuf};
use std::process::Command;

pub trait ThumbnailSource {
    fn thumbnails(
        &self,
        media: &MediaInfo,
        count: usize,
        out_dir: &Path,
    ) -> Result<Vec<PathBuf>, ProbeError>;
}

pub struct FfmpegThumbs;

impl ThumbnailSource for FfmpegThumbs {
    fn thumbnails(
        &self,
        media: &MediaInfo,
        count: usize,
        out_dir: &Path,
    ) -> Result<Vec<PathBuf>, ProbeError> {
        std::fs::create_dir_all(out_dir)?;
        let mut paths = Vec::new();
        for i in 0..count {
            let t = media.duration_s * (i as f64 + 0.5) / count as f64;
            let out = out_dir.join(format!("thumb_{i:03}.jpg"));
            let status = Command::new("ffmpeg")
                .args(["-hide_banner", "-loglevel", "error", "-y", "-ss"])
                .arg(format!("{t:.3}"))
                .arg("-i")
                .arg(&media.path)
                .args(["-frames:v", "1", "-vf", "scale=-2:96"])
                .arg(&out)
                .status()?;
            if status.success() {
                paths.push(out);
            }
        }
        Ok(paths)
    }
}
