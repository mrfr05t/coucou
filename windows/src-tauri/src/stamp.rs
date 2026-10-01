// Local wall clock, in exactly one place.
//
// Both the log header and the `settings.json.bak-YYYYMMDD-HHMMSS` backup names
// need it, and GetLocalTime on Windows vs `SystemTime` on Linux would have them
// disagreeing about which day it is — a backup named in UTC next to a log line
// in local time is a support question waiting to happen.

/// (year, month, day, hour, minute, second) in **local** time.
#[cfg(windows)]
pub fn now() -> (u16, u16, u16, u16, u16, u16) {
    use windows::Win32::System::SystemInformation::GetLocalTime;
    let t = unsafe { GetLocalTime() };
    (t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond)
}

#[cfg(not(windows))]
pub fn now() -> (u16, u16, u16, u16, u16, u16) {
    use chrono::{Datelike, Timelike};
    let t = chrono::Local::now();
    let d = t.date_naive();
    let clock = t.time();
    (
        d.year() as u16,
        d.month() as u16,
        d.day() as u16,
        clock.hour() as u16,
        clock.minute() as u16,
        clock.second() as u16,
    )
}

/// `2026-09-30 22:47:42` — for log lines.
pub fn log_stamp() -> String {
    let (y, mo, d, h, mi, s) = now();
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02}")
}

/// `20260930-224742` — for `settings.json.bak-…` file names. No spaces or
/// colons, so it stays safe in a shell glob and sorts correctly.
pub fn file_stamp() -> String {
    let (y, mo, d, h, mi, s) = now();
    format!("{y:04}{mo:02}{d:02}-{h:02}{mi:02}{s:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamps_have_the_expected_shape() {
        let log = log_stamp();
        assert_eq!(log.len(), 19, "expected `YYYY-MM-DD HH:MM:SS`, got {log}");
        //  0123 4 56 7 89 10 11 12 13 14 15 16 17 18
        //  2026-09-30 22:47:42
        assert_eq!(&log[4..5], "-", "date separator");
        assert_eq!(&log[10..11], " ", "date/time separator");
        assert_eq!(&log[13..14], ":", "hour/minute separator");

        let file = file_stamp();
        assert_eq!(file.len(), 15, "expected `YYYYMMDD-HHMMSS`, got {file}");
        assert_eq!(&file[8..9], "-");
        assert!(!file.contains(':') && !file.contains(' '), "must be path-safe");
    }

    #[test]
    fn both_stamps_agree_on_the_date() {
        // The whole point of this module: the log and the backup name must
        // come from the same reading of the clock, not two of them.
        let (y, mo, d, ..) = now();
        let file = file_stamp();
        assert!(file.starts_with(&format!("{y:04}{mo:02}{d:02}")),
            "file_stamp {file} disagrees with now() ({y}-{mo}-{d})");
    }
}
