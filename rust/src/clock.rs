//! Ortszeit für die Zeitstempel in der Konsole – ohne Zeitzonen-Bibliothek.
//!
//! `SystemTime` kennt nur UTC; den Zeitzonen-Versatz kann nur das Betriebssystem liefern.
//! Genau das wird hier direkt abgefragt: `GetLocalTime` unter Windows, `localtime_r` sonst.
//! Das kostet keine zusätzliche Abhängigkeit (windows-sys bzw. libc sind ohnehin dabei).

/// Ortszeit als (Stunde, Minute, Sekunde).
#[cfg(windows)]
pub fn local_hms() -> (u8, u8, u8) {
    use windows_sys::Win32::Foundation::SYSTEMTIME;
    let mut st: SYSTEMTIME = unsafe { std::mem::zeroed() };
    unsafe { windows_sys::Win32::System::SystemInformation::GetLocalTime(&mut st) };
    (st.wHour as u8, st.wMinute as u8, st.wSecond as u8)
}

#[cfg(not(windows))]
pub fn local_hms() -> (u8, u8, u8) {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0) as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // localtime_r wertet TZ bzw. /etc/localtime aus und braucht keinen globalen Zustand.
    if unsafe { libc::localtime_r(&secs, &mut tm) }.is_null() {
        return (0, 0, 0);
    }
    (tm.tm_hour as u8, tm.tm_min as u8, tm.tm_sec as u8)
}

/// "HH:MM" der Ortszeit.
pub fn hhmm() -> String {
    let (h, m, _) = local_hms();
    format!("{:02}:{:02}", h, m)
}

#[cfg(test)]
mod tests {
    /// Prüft, dass die plattformabhängige Abfrage überhaupt eine gültige Uhrzeit liefert
    /// (unter Linux geht sie über libc, unter Windows über GetLocalTime).
    #[test]
    fn ortszeit_ist_plausibel() {
        let (hour, minute, second) = super::local_hms();
        assert!(hour < 24 && minute < 60 && second < 60, "{}:{}:{}", hour, minute, second);
        assert_eq!(super::hhmm().len(), 5);
    }
}
