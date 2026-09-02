//! Bundled process-isolated read-only scan helper.

fn main() {
    let mut args = std::env::args_os().skip(1);
    let result = match args.next() {
        None => chippytea_core::scan_worker::run_stdio(),
        Some(mode) if mode == "--editor-review" => match (args.next(), args.next()) {
            (Some(editor), None) => match editor.to_str() {
                Some(editor) => chippytea_core::scan_worker::run_editor_review(editor),
                None => Err("Editor review name is not UTF-8".into()),
            },
            _ => Err("Editor review requires one fixed editor name".into()),
        },
        _ => Err("Unknown scan-helper mode".into()),
    };
    if let Err(error) = result {
        let message = chippytea_core::scan_worker::bounded_diagnostic(&error.to_string());
        eprintln!("{message}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use chippytea_core::scan_worker::prepare_scan_process;
    use std::{io, process::Command};

    // Keep process-spawning limit tests in this standalone test binary. A
    // concurrent fork can temporarily inherit an unrelated library test's
    // lock/socket descriptors until exec closes them, invalidating an immediate
    // reopen or peer-close assertion even when FD_CLOEXEC is correctly set.
    const SCAN_PROCESS_DESCRIPTOR_LIMIT: libc::rlim_t = 864;

    fn descriptor_limits() -> io::Result<libc::rlimit> {
        let mut limits = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limits) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(limits)
    }

    #[test]
    fn scan_descriptor_reservation_is_process_local_and_honors_hard_limits() {
        const CHILD_MODE: &str = "CHIPPYTEA_TEST_SCAN_DESCRIPTOR_RESERVE";
        let original = descriptor_limits().unwrap();
        if let Ok(mode) = std::env::var(CHILD_MODE) {
            // Only this short-lived child changes limits. Keep some existing
            // descriptors alive to exercise the non-traversal reserve too.
            let held = (0..32)
                .map(|_| std::fs::File::open("/dev/null").unwrap())
                .collect::<Vec<_>>();
            let limits = match mode.as_str() {
                "raise" => libc::rlimit {
                    rlim_cur: 256,
                    rlim_max: original.rlim_max,
                },
                "sufficient" => libc::rlimit {
                    rlim_cur: (SCAN_PROCESS_DESCRIPTOR_LIMIT + 17).min(original.rlim_max),
                    rlim_max: original.rlim_max,
                },
                "restricted" => libc::rlimit {
                    rlim_cur: 128.min(original.rlim_max),
                    rlim_max: 256.min(original.rlim_max),
                },
                _ => panic!("Unknown descriptor-reservation child mode"),
            };
            assert_eq!(unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &limits) }, 0);
            let result = prepare_scan_process();
            let after = descriptor_limits().unwrap();
            assert_eq!(after.rlim_max, limits.rlim_max);
            if mode == "restricted" {
                assert!(result.unwrap_err().contains("hard limit"));
                assert_eq!(after.rlim_cur, limits.rlim_cur);
            } else {
                result.unwrap();
                assert_eq!(
                    after.rlim_cur,
                    limits.rlim_cur.max(SCAN_PROCESS_DESCRIPTOR_LIMIT)
                );
                // A repeated reservation never lowers a sufficient limit.
                prepare_scan_process().unwrap();
                assert_eq!(descriptor_limits().unwrap().rlim_cur, after.rlim_cur);
            }
            assert!(held.iter().all(|file| file.metadata().is_ok()));
            return;
        }

        let modes: &[&str] = if original.rlim_max >= SCAN_PROCESS_DESCRIPTOR_LIMIT {
            &["raise", "sufficient", "restricted"]
        } else {
            // An intentionally restricted test host cannot grant the budget;
            // still prove that refusal does not mutate its hard or soft limit.
            &["restricted"]
        };
        for mode in modes {
            let output = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "tests::scan_descriptor_reservation_is_process_local_and_honors_hard_limits",
                    "--nocapture",
                ])
                .env(CHILD_MODE, mode)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "descriptor-reservation subprocess {mode} failed: {}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            let after = descriptor_limits().unwrap();
            assert_eq!(after.rlim_cur, original.rlim_cur);
            assert_eq!(after.rlim_max, original.rlim_max);
        }
    }
}
