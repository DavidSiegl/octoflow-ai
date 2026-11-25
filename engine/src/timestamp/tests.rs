use super::*;

#[test]
fn times_are_written_and_read_as_rfc_3339() {
    assert_eq!(iso_utc(0), "1970-01-01T00:00:00+00:00");
    assert_eq!(iso_utc(1_791_000_000), "2026-10-03T04:00:00+00:00");
    assert_eq!(iso_utc(951_782_400), "2000-02-29T00:00:00+00:00");
    assert_eq!(iso_utc(-1), "1969-12-31T23:59:59+00:00");
    for seconds in [0, 1_791_000_000, 951_782_400, 4_102_444_799, -86_400] {
        assert_eq!(epoch_seconds(&iso_utc(seconds)), Ok(seconds));
    }
    assert_eq!(epoch_seconds("2026-10-03T06:00:00+02:00"), Ok(1_791_000_000));
    assert_eq!(epoch_seconds("2026-10-03T04:00:00.999Z"), Ok(1_791_000_000));
    assert_eq!(epoch_time("2026-10-03T04:00:00.25Z"), Ok(1_791_000_000.25));
    // Whole seconds are those before the time, before the epoch too.
    assert_eq!(epoch_seconds("1969-12-31T23:59:59.5Z"), Ok(-1));
    for bad in
        ["<time>", "garbled", "2026-13-01T00:00:00Z", "2026-02-30T00:00:00Z", "2026-10-03T04:00:00", "2026-10-03"]
    {
        assert!(epoch_seconds(bad).is_err(), "{bad}");
    }
}
