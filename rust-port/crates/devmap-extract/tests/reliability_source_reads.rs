//! Source limits must hold at the actual read, including replacement with a FIFO.

#[cfg(test)]
mod reliability_source_reads {
    #[test]
    fn ra4_valid_source_at_the_exact_limit_is_retained() {
        let path =
            std::env::temp_dir().join(format!("devmap-source-limit-{}.py", std::process::id()));
        let source = "#".repeat(devmap_extract::MAX_SOURCE_BYTES as usize);
        std::fs::write(&path, &source).unwrap();
        assert_eq!(devmap_extract::read_source(&path).unwrap(), source);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    #[cfg(unix)]
    fn ra4_fifo_is_refused_without_waiting_for_a_writer() {
        const PROBE: &str = "DEVMAP_AUDIT_FIFO_PROBE";
        if let Some(path) = std::env::var_os(PROBE) {
            assert!(devmap_extract::read_source(std::path::Path::new(&path)).is_err());
            return;
        }
        let path =
            std::env::temp_dir().join(format!("devmap-source-fifo-{}.py", std::process::id()));
        use devmap_extract::subprocess::{run_bounded, Bounds};
        let bounds = || Bounds {
            deadline: std::time::Duration::from_secs(5),
            stdout_cap: 4096,
            stderr_cap: 4096,
        };
        let created =
            run_bounded(std::process::Command::new("mkfifo").arg(&path), bounds()).unwrap();
        assert!(created.status.success(), "{}", created.stderr_trimmed());
        let outcome = run_bounded(
            std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "reliability_source_reads::ra4_fifo_is_refused_without_waiting_for_a_writer",
                ])
                .env(PROBE, &path),
            bounds(),
        );
        std::fs::remove_file(&path).unwrap();
        let output = outcome.expect("opening a candidate FIFO must not wait for a writer");
        assert!(output.status.success(), "{}", output.stderr_trimmed());
    }

    #[test]
    fn ra4_the_read_itself_enforces_the_discovery_size_ceiling() {
        let path = std::env::temp_dir().join(format!(
            "devmap-source-read-{}-{}.py",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(
            &path,
            "x".repeat(devmap_extract::MAX_SOURCE_BYTES as usize + 1),
        )
        .unwrap();
        let result = devmap_extract::read_source(&path);
        std::fs::remove_file(path).unwrap();
        assert!(
            result.is_err(),
            "a file that grew after discovery escaped the byte ceiling"
        );
    }
}
