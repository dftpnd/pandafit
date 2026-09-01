use std::path::PathBuf;
use std::process::Command;

#[derive(Debug, Clone, PartialEq)]
pub struct CheckResult {
    pub name: String,
    pub ok: bool,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PreflightReport {
    pub checks: Vec<CheckResult>,
}

impl PreflightReport {
    pub fn is_ok(&self) -> bool {
        self.checks.iter().all(|c| c.ok)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PreflightInput {
    pub needs_dv_tools: bool,
    pub needs_nvenc: bool,
    pub needs_burner: bool,
    pub required_free_bytes: u64,
    pub workdir: PathBuf,
}

fn have(program: &str) -> bool {
    Command::new(program)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn tool_check(program: &str, hint: &str) -> CheckResult {
    let ok = have(program);
    CheckResult {
        name: format!("утилита {program}"),
        ok,
        detail: if ok { "найдена".into() } else { hint.to_string() },
    }
}

fn free_bytes(path: &std::path::Path) -> Option<u64> {
    let out = Command::new("df").args(["-B1", "--output=avail"]).arg(path).output().ok()?;
    String::from_utf8_lossy(&out.stdout).lines().nth(1)?.trim().parse().ok()
}

pub fn preflight(input: &PreflightInput) -> PreflightReport {
    let mut checks = vec![
        tool_check("ffmpeg", "установите пакет ffmpeg"),
        tool_check("ffprobe", "установите пакет ffmpeg"),
    ];

    if input.needs_dv_tools {
        checks.push(tool_check("dovi_tool", "cargo install dovi_tool"));
        checks.push(tool_check("mkvmerge", "sudo apt install mkvtoolnix"));
    }
    if input.needs_burner {
        checks.push(tool_check("growisofs", "sudo apt install dvd+rw-tools"));
    }

    if input.needs_nvenc {
        let ok = Command::new("ffmpeg")
            .args(["-hide_banner", "-encoders"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).contains("hevc_nvenc"))
            .unwrap_or(false);
        checks.push(CheckResult {
            name: "кодирование на видеокарте (NVENC)".into(),
            ok,
            detail: if ok {
                "доступно".into()
            } else {
                "ffmpeg собран без NVENC — выберите кодирование на процессоре".into()
            },
        });
    }

    let avail = free_bytes(&input.workdir);
    let ok = avail.is_some_and(|a| a >= input.required_free_bytes);
    checks.push(CheckResult {
        name: "свободное место".into(),
        ok,
        detail: match avail {
            Some(a) => format!(
                "нужно {:.1} ГБ, доступно {:.1} ГБ",
                input.required_free_bytes as f64 / 1e9,
                a as f64 / 1e9
            ),
            None => "не удалось определить".into(),
        },
    });

    PreflightReport { checks }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> PreflightInput {
        PreflightInput {
            needs_dv_tools: false,
            needs_nvenc: false,
            needs_burner: false,
            required_free_bytes: 1,
            workdir: std::env::temp_dir(),
        }
    }

    #[test]
    fn ffmpeg_and_ffprobe_are_always_checked() {
        let r = preflight(&input());
        assert!(r.checks.iter().any(|c| c.name.contains("ffmpeg")));
        assert!(r.checks.iter().any(|c| c.name.contains("ffprobe")));
    }

    #[test]
    fn dv_tools_are_only_checked_when_the_chain_needs_them() {
        let r = preflight(&input());
        assert!(!r.checks.iter().any(|c| c.name.contains("dovi_tool")));
        let r = preflight(&PreflightInput { needs_dv_tools: true, ..input() });
        assert!(r.checks.iter().any(|c| c.name.contains("dovi_tool")));
        assert!(r.checks.iter().any(|c| c.name.contains("mkvmerge")));
    }

    #[test]
    fn missing_tool_carries_an_install_hint() {
        let r = preflight(&PreflightInput { needs_dv_tools: true, ..input() });
        let c = r.checks.iter().find(|c| c.name.contains("dovi_tool")).unwrap();
        if !c.ok {
            assert!(c.detail.contains("cargo install") || c.detail.contains("apt"));
        }
    }

    #[test]
    fn impossible_free_space_requirement_fails_the_report() {
        let r = preflight(&PreflightInput { required_free_bytes: u64::MAX, ..input() });
        assert!(!r.is_ok());
        assert!(r.checks.iter().any(|c| !c.ok && c.name.contains("место")));
    }
}
