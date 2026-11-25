//! Times as the engine records them: RFC 3339 text in UTC (state.json, the journals, metrics.json) and the seconds
//! since the epoch it computes with. A leaf: it imports nothing of the crate, so any module may use it.

use jiff::Timestamp;

/// seconds since the epoch as an RFC 3339 time in UTC to the second, with its offset written out:
/// 2026-10-28T08:34:31+00:00. The engine writes every time it records so.
pub fn iso_utc(seconds: i64) -> String {
    // Seconds beyond jiff's years -9999 to 9999 are clamped to its bounds; no clock gives them.
    let seconds = seconds.clamp(Timestamp::MIN.as_second(), Timestamp::MAX.as_second());
    let at = Timestamp::from_second(seconds).unwrap_or(Timestamp::UNIX_EPOCH);
    at.strftime("%Y-%m-%dT%H:%M:%S+00:00").to_string()
}

/// The seconds since the epoch, with their fraction to the microsecond, of an ISO 8601 time with an offset (Z, +hh:mm
/// or +hh), as jiff's Timestamp reads it: RFC 3339 and ISO 8601's basic format among its forms. Errors: iso is not
/// such a time, a time without an offset among them, as jiff words it.
pub fn epoch_time(iso: &str) -> Result<f64, String> {
    let at: Timestamp = iso.parse().map_err(|e: jiff::Error| e.to_string())?;
    Ok(at.as_microsecond() as f64 / 1e6)
}

/// epoch_time's whole seconds, the fraction cut off; errors as its.
pub fn epoch_seconds(iso: &str) -> Result<i64, String> {
    epoch_time(iso).map(|seconds| seconds.floor() as i64)
}

#[cfg(test)]
mod tests;
