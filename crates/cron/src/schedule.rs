//! Next-run computation for all schedule kinds.

use {
    anyhow::{Result, bail},
    chrono::{DateTime, TimeZone, Utc},
    cron::Schedule,
};

use crate::{parse, types::CronSchedule};

/// Compute the next run time (epoch millis) for a given schedule.
///
/// Returns `None` if the schedule has no future runs (e.g. a past one-shot).
pub fn compute_next_run(schedule: &CronSchedule, now_ms: u64) -> Result<Option<u64>> {
    match schedule {
        CronSchedule::Once { at } => {
            let at_ms = parse::parse_absolute_time_ms(at)?;
            if at_ms > now_ms {
                Ok(Some(at_ms))
            } else {
                Ok(None) // already past
            }
        },
        CronSchedule::Every { every } => {
            let every_ms = parse::parse_duration_ms(every)?;
            if every_ms == 0 {
                bail!("every must be > 0");
            }
            Ok(Some(now_ms + every_ms))
        },
        CronSchedule::Cron { expr, timezone } => {
            let schedule: Schedule = expr
                .parse()
                .or_else(|_| {
                    // The `cron` crate requires 7 fields (sec min hour dom month dow year).
                    // Users typically provide 5 fields (min hour dom month dow).
                    // Prepend "0" for seconds and append "*" for year.
                    let padded = format!("0 {expr} *");
                    padded.parse::<Schedule>()
                })
                .map_err(|e| anyhow::anyhow!("invalid cron expression '{expr}': {e}"))?;

            let now_dt = DateTime::from_timestamp_millis(now_ms as i64)
                .unwrap_or_else(|| Utc.timestamp_millis_opt(0).unwrap());

            let tz: chrono_tz::Tz = timezone
                .parse()
                .map_err(|_| anyhow::anyhow!("unknown timezone: {timezone}"))?;
            let now_local = now_dt.with_timezone(&tz);
            let next = schedule
                .after(&now_local)
                .next()
                .map(|dt| dt.timestamp_millis() as u64);

            Ok(next)
        },
    }
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_at_future() {
        let s = CronSchedule::Once {
            at: "1970-01-01T00:00:02Z".into(),
        };
        assert_eq!(compute_next_run(&s, 1000).unwrap(), Some(2000));
    }

    #[test]
    fn test_at_past() {
        let s = CronSchedule::Once {
            at: "1970-01-01T00:00:00.500Z".into(),
        };
        assert_eq!(compute_next_run(&s, 1000).unwrap(), None);
    }

    #[test]
    fn test_every_no_anchor() {
        let s = CronSchedule::Every { every: "60s".into() };
        let now = 100_000;
        let next = compute_next_run(&s, now).unwrap().unwrap();
        // Next = now + 60_000
        assert_eq!(next, now + 60_000);
    }

    #[test]
    fn test_every_zero_interval() {
        let s = CronSchedule::Every { every: "0s".into() };
        assert!(compute_next_run(&s, 1000).is_err());
    }

    #[test]
    fn test_cron_five_field() {
        let s = CronSchedule::Cron {
            expr: "0 9 * * *".into(),
            timezone: "UTC".into(),
        };
        let now_ms = 1_706_745_600_000; // 2024-02-01T00:00:00Z
        let next = compute_next_run(&s, now_ms).unwrap().unwrap();
        assert!(next > now_ms);
        // Should be 9:00 UTC on 2024-02-01
        let dt = DateTime::from_timestamp_millis(next as i64).unwrap();
        assert_eq!(dt.format("%H:%M").to_string(), "09:00");
    }

    #[test]
    fn test_cron_with_timezone() {
        let s = CronSchedule::Cron {
            expr: "0 9 * * *".into(),
            timezone: "Europe/Paris".into(),
        };
        let now_ms = 1_706_745_600_000; // 2024-02-01T00:00:00Z
        let next = compute_next_run(&s, now_ms).unwrap().unwrap();
        assert!(next > now_ms);
        // 9:00 Paris = 08:00 UTC in winter (CET = UTC+1)
        let dt = DateTime::from_timestamp_millis(next as i64).unwrap();
        assert_eq!(dt.format("%H:%M").to_string(), "08:00");
    }

    #[test]
    fn test_cron_invalid_expr() {
        let s = CronSchedule::Cron {
            expr: "not valid".into(),
            timezone: "UTC".into(),
        };
        assert!(compute_next_run(&s, 1000).is_err());
    }

    #[test]
    fn test_cron_invalid_tz() {
        let s = CronSchedule::Cron {
            expr: "0 9 * * *".into(),
            timezone: "Mars/Olympus".into(),
        };
        assert!(compute_next_run(&s, 1000).is_err());
    }
}
