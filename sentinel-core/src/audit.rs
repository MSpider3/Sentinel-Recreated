use anyhow::{Context, Result};
use chrono::{Duration, Local, NaiveDate, Utc};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// Audit record matching spec format:
/// TIMESTAMP|USER|RESULT|DISTANCE|TIER|LIVENESS_STATUS|SPOOF_SCORE|DURATION_MS
#[derive(Debug, Clone)]
pub struct AuditRecord {
    pub timestamp: String,
    pub user: String,
    pub result: String,
    pub distance: String,
    pub tier: String,
    pub liveness_status: String,
    pub spoof_score: String,
    pub duration_ms: u64,
}

impl AuditRecord {
    pub fn new_now(
        user: impl Into<String>,
        result: impl Into<String>,
        distance: Option<f32>,
        tier: u32,
        liveness_status: impl Into<String>,
        spoof_score: Option<f32>,
        duration_ms: u64,
    ) -> Self {
        let timestamp = Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();
        let distance = distance
            .map(|d| format!("{:.4}", d))
            .unwrap_or_else(|| "N/A".to_string());
        let spoof_score = spoof_score
            .map(|s| format!("{:.3}", s))
            .unwrap_or_else(|| "N/A".to_string());

        Self {
            timestamp,
            user: user.into(),
            result: result.into(),
            distance,
            tier: tier.to_string(),
            liveness_status: liveness_status.into(),
            spoof_score,
            duration_ms,
        }
    }

    pub fn to_line(&self) -> String {
        format!(
            "{}|{}|{}|{}|{}|{}|{}|{}\n",
            self.timestamp,
            self.user,
            self.result,
            self.distance,
            self.tier,
            self.liveness_status,
            self.spoof_score,
            self.duration_ms
        )
    }
}

pub struct AuditLogger {
    pub dir: PathBuf,
}

impl Default for AuditLogger {
    fn default() -> Self {
        Self::new()
    }
}

impl AuditLogger {
    pub fn new() -> Self {
        let dir = PathBuf::from("/var/log/sentinel");
        let logger = Self { dir };
        let _ = logger.init_dir_and_cleanup();
        logger
    }

    pub fn with_custom_dir(dir: impl AsRef<Path>) -> Self {
        let logger = Self {
            dir: dir.as_ref().to_path_buf(),
        };
        let _ = logger.init_dir_and_cleanup();
        logger
    }

    pub fn init_dir_and_cleanup(&self) -> Result<()> {
        if !self.dir.exists() {
            fs::create_dir_all(&self.dir)
                .with_context(|| format!("Failed to create log dir: {}", self.dir.display()))?;
            #[cfg(unix)]
            {
                let mut perms = fs::metadata(&self.dir)?.permissions();
                perms.set_mode(0o755);
                fs::set_permissions(&self.dir, perms).ok();
            }
        }
        self.cleanup_old_logs(30)?;
        Ok(())
    }

    /// Delete any `auth_YYYY-MM-DD.log` files older than `max_days` days.
    pub fn cleanup_old_logs(&self, max_days: i64) -> Result<()> {
        if !self.dir.exists() {
            return Ok(());
        }

        let today = Local::now().date_naive();
        let cutoff = today - Duration::days(max_days);

        let entries = fs::read_dir(&self.dir)?;
        for entry in entries.flatten() {
            let path = entry.path();
            if let Some(filename) = path.file_name().and_then(|n| n.to_str()) {
                if filename.starts_with("auth_") && filename.ends_with(".log") {
                    let date_part = filename
                        .trim_start_matches("auth_")
                        .trim_end_matches(".log");
                    if let Ok(file_date) = NaiveDate::parse_from_str(date_part, "%Y-%m-%d") {
                        if file_date < cutoff {
                            let _ = fs::remove_file(&path);
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Log a record to `/var/log/sentinel/auth_YYYY-MM-DD.log`
    pub fn log(&self, record: &AuditRecord) -> Result<()> {
        let _ = self.init_dir_and_cleanup();
        let today = Local::now().format("%Y-%m-%d").to_string();
        let file_path = self.dir.join(format!("auth_{}.log", today));

        let line = record.to_line();

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&file_path)
            .with_context(|| format!("Failed to open log file: {}", file_path.display()))?;

        file.write_all(line.as_bytes())?;

        #[cfg(unix)]
        {
            if let Ok(m) = fs::metadata(&file_path) {
                let mut perms = m.permissions();
                perms.set_mode(0o640);
                fs::set_permissions(&file_path, perms).ok();
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_audit_record_format() {
        let record = AuditRecord {
            timestamp: "2026-07-25T10:32:10.104Z".to_string(),
            user: "testuser".to_string(),
            result: "GRANTED".to_string(),
            distance: "0.1145".to_string(),
            tier: "1".to_string(),
            liveness_status: "SKIPPED".to_string(),
            spoof_score: "0.961".to_string(),
            duration_ms: 38,
        };
        let line = record.to_line();
        assert_eq!(
            line,
            "2026-07-25T10:32:10.104Z|testuser|GRANTED|0.1145|1|SKIPPED|0.961|38\n"
        );
    }

    #[test]
    fn test_audit_logger_and_retention() {
        let tmp_dir = std::env::temp_dir().join("sentinel_test_audit");
        let logger = AuditLogger::with_custom_dir(&tmp_dir);

        let record = AuditRecord::new_now(
            "testuser",
            "GRANTED",
            Some(0.1145),
            1,
            "SKIPPED",
            Some(0.961),
            38,
        );

        logger.log(&record).unwrap();

        // Create a mock 40-day old log file
        let old_file = tmp_dir.join("auth_2020-01-01.log");
        fs::write(&old_file, "old log entry").unwrap();
        assert!(old_file.exists());

        // Run cleanup
        logger.cleanup_old_logs(30).unwrap();
        assert!(!old_file.exists());

        let _ = fs::remove_dir_all(&tmp_dir);
    }
}
