use crate::progress::{parse_ffmpeg_progress, parse_growisofs_line, FfmpegProgressState};
use pandafit_core::compile::{ProgressKind, Step};
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const STDERR_TAIL_LINES: usize = 200;

#[derive(Debug, Clone, PartialEq)]
pub enum ProgressEvent {
    Started { step_id: &'static str, title: String },
    Tick { step_id: &'static str, position_s: f64, bytes_written: u64, speed: Option<f64> },
    Log { step_id: &'static str, line: String },
    Finished { step_id: &'static str },
    Failed { step_id: &'static str, message: String, tail: Vec<String> },
    Cancelled,
}

pub trait JobRunner {
    fn run(&self, steps: Vec<Step>, tx: Sender<ProgressEvent>, cancel: Arc<AtomicBool>);
}

pub struct ProcessRunner;

fn terminate(child: &mut Child) {
    #[cfg(unix)]
    unsafe {
        libc::kill(child.id() as i32, libc::SIGTERM);
    }
    for _ in 0..50 {
        if matches!(child.try_wait(), Ok(Some(_))) {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = child.kill();
}

impl JobRunner for ProcessRunner {
    fn run(&self, steps: Vec<Step>, tx: Sender<ProgressEvent>, cancel: Arc<AtomicBool>) {
        for step in steps {
            if cancel.load(Ordering::SeqCst) {
                let _ = tx.send(ProgressEvent::Cancelled);
                return;
            }

            if let Some(p) = &step.produces {
                if p.exists() && p.metadata().map(|m| m.len()).unwrap_or(0) > 0 {
                    let _ = tx.send(ProgressEvent::Log {
                        step_id: step.id,
                        line: format!("пропущен: {} уже готов", p.display()),
                    });
                    let _ = tx.send(ProgressEvent::Finished { step_id: step.id });
                    continue;
                }
            }

            if let Some(prepared) = &step.prepare {
                if let Some(parent) = prepared.path.parent() {
                    if let Err(e) = std::fs::create_dir_all(parent) {
                        let _ = tx.send(ProgressEvent::Failed {
                            step_id: step.id,
                            message: format!(
                                "не удалось создать папку {}: {e}",
                                parent.display()
                            ),
                            tail: vec![],
                        });
                        return;
                    }
                }
                if let Err(e) = std::fs::write(&prepared.path, &prepared.contents) {
                    let _ = tx.send(ProgressEvent::Failed {
                        step_id: step.id,
                        message: format!(
                            "не удалось записать конфигурацию {}: {e}",
                            prepared.path.display()
                        ),
                        tail: vec![],
                    });
                    return;
                }
            }

            let _ = tx.send(ProgressEvent::Started {
                step_id: step.id,
                title: step.title.clone(),
            });

            let spawned = Command::new(&step.program)
                .args(&step.args)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn();

            let mut child = match spawned {
                Ok(c) => c,
                Err(e) => {
                    let _ = tx.send(ProgressEvent::Failed {
                        step_id: step.id,
                        message: format!("не удалось запустить {}: {e}", step.program),
                        tail: vec![],
                    });
                    return;
                }
            };

            let tail = Arc::new(Mutex::new(Vec::<String>::new()));
            let stdout = child.stdout.take();
            let stderr = child.stderr.take();

            let out_tx = tx.clone();
            let step_id = step.id;
            let kind = step.progress;
            let h_out = std::thread::spawn(move || {
                let Some(stdout) = stdout else { return };
                let mut st = FfmpegProgressState::default();
                for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                    if kind == ProgressKind::FfmpegPipe {
                        if let Some(t) = parse_ffmpeg_progress(&mut st, &line) {
                            let _ = out_tx.send(ProgressEvent::Tick {
                                step_id,
                                position_s: t.position_s,
                                bytes_written: t.bytes_written,
                                speed: t.speed,
                            });
                        }
                    }
                }
            });

            let err_tx = tx.clone();
            let tail_c = tail.clone();
            let h_err = std::thread::spawn(move || {
                let Some(stderr) = stderr else { return };
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    if kind == ProgressKind::Growisofs {
                        if let Some(pct) = parse_growisofs_line(&line) {
                            let _ = err_tx.send(ProgressEvent::Tick {
                                step_id,
                                position_s: pct,
                                bytes_written: 0,
                                speed: None,
                            });
                        }
                    }
                    let mut t = tail_c.lock().unwrap();
                    if t.len() == STDERR_TAIL_LINES {
                        t.remove(0);
                    }
                    t.push(line.clone());
                    let _ = err_tx.send(ProgressEvent::Log { step_id, line });
                }
            });

            let status = loop {
                if cancel.load(Ordering::SeqCst) {
                    terminate(&mut child);
                    let _ = h_out.join();
                    let _ = h_err.join();
                    let _ = tx.send(ProgressEvent::Cancelled);
                    return;
                }
                match child.try_wait() {
                    Ok(Some(s)) => break s,
                    Ok(None) => std::thread::sleep(Duration::from_millis(100)),
                    Err(e) => {
                        let _ = tx.send(ProgressEvent::Failed {
                            step_id: step.id,
                            message: e.to_string(),
                            tail: vec![],
                        });
                        return;
                    }
                }
            };

            let _ = h_out.join();
            let _ = h_err.join();

            if status.success() {
                let _ = tx.send(ProgressEvent::Finished { step_id: step.id });
            } else {
                let tail = tail.lock().unwrap().clone();
                let _ = tx.send(ProgressEvent::Failed {
                    step_id: step.id,
                    message: format!("{} завершился с кодом {status}", step.program),
                    tail,
                });
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pandafit_core::compile::{PreparedFile, ProgressKind, Step};
    use std::sync::atomic::AtomicBool;
    use std::sync::mpsc;
    use std::sync::Arc;

    fn step(program: &str, args: &[&str]) -> Step {
        Step {
            id: "build",
            title: "тест".into(),
            program: program.into(),
            args: args.iter().map(|s| s.to_string()).collect(),
            progress: ProgressKind::None,
            produces: None,
            prepare: None,
        }
    }

    #[test]
    fn successful_step_reports_started_then_finished() {
        let (tx, rx) = mpsc::channel();
        ProcessRunner.run(vec![step("true", &[])], tx, Arc::new(AtomicBool::new(false)));
        let events: Vec<_> = rx.iter().collect();
        assert!(matches!(events.first(), Some(ProgressEvent::Started { .. })));
        assert!(matches!(events.last(), Some(ProgressEvent::Finished { .. })));
    }

    #[test]
    fn failing_step_reports_failure_with_stderr_tail() {
        let (tx, rx) = mpsc::channel();
        ProcessRunner.run(
            vec![step("sh", &["-c", "echo беда 1>&2; exit 3"])],
            tx,
            Arc::new(AtomicBool::new(false)),
        );
        let events: Vec<_> = rx.iter().collect();
        match events.last() {
            Some(ProgressEvent::Failed { tail, .. }) => {
                assert!(tail.iter().any(|l| l.contains("беда")))
            }
            other => panic!("ожидали Failed, получили {other:?}"),
        }
    }

    #[test]
    fn a_failing_step_stops_the_chain() {
        let (tx, rx) = mpsc::channel();
        ProcessRunner.run(
            vec![step("false", &[]), step("true", &[])],
            tx,
            Arc::new(AtomicBool::new(false)),
        );
        let events: Vec<_> = rx.iter().collect();
        let starts = events.iter().filter(|e| matches!(e, ProgressEvent::Started { .. })).count();
        assert_eq!(starts, 1, "второй шаг не должен был запуститься");
    }

    #[test]
    fn cancellation_flag_stops_a_long_running_step() {
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let c = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(200));
            c.store(true, std::sync::atomic::Ordering::SeqCst);
        });
        let started = std::time::Instant::now();
        ProcessRunner.run(vec![step("sleep", &["30"])], tx, cancel);
        assert!(started.elapsed().as_secs() < 10, "отмена не сработала");
        let events: Vec<_> = rx.iter().collect();
        assert!(events.iter().any(|e| matches!(e, ProgressEvent::Cancelled)));
    }

    #[test]
    fn prepared_file_is_written_to_disk_before_the_command_runs() {
        let dir = std::env::temp_dir().join(format!("pandafit-prepare-test-{}", std::process::id()));
        let path = dir.join("config.xml");
        let mut s = step("cat", &[path.to_str().unwrap()]);
        s.prepare = Some(PreparedFile {
            path: path.clone(),
            contents: "содержимое конфига".into(),
        });
        let (tx, rx) = mpsc::channel();
        ProcessRunner.run(vec![s], tx, Arc::new(AtomicBool::new(false)));
        let events: Vec<_> = rx.iter().collect();
        assert!(matches!(events.last(), Some(ProgressEvent::Finished { .. })), "{events:?}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "содержимое конфига");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
