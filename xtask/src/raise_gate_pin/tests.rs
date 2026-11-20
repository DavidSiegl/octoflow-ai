//! raise-gate-pin's copies of the engine's gate facts, held to the engine's own. The xtask does not depend on the
//! engine to run (xtask/Cargo.toml's dev-dependencies), so these tests are what keeps the two from drifting.

use octoflow_engine::local_gate;

use super::*;

#[test]
fn the_first_release_with_gate_is_the_engine_s() {
    let (major, minor) = local_gate::FIRST_WITH_GATE;
    assert_eq!(FIRST_WITH_GATE, Version(major, minor, 0));
}

#[test]
fn a_release_is_downloaded_from_where_the_engine_downloads_it() {
    for version in [FIRST_WITH_GATE, Version(1, 12, 3)] {
        assert_eq!(release_url(version), local_gate::RELEASE_URL.replace("{v}", &version.to_string()));
    }
}

#[test]
fn a_pin_reads_as_the_engine_reads_it() {
    let sha = "de6173ed3264d1ea635f03d4240c5de1b02f0c5122e37e8a39b5ce5010d4e106";
    let commented = format!("# a comment\n#0.1.0 {sha}\n0.9.2 {sha}\n");
    for pin in [include_str!("../../../ci/gate.pin"), &commented] {
        let (version, sha) = pin_of(pin).unwrap();
        let (engine_version, engine_sha) = local_gate::parse_pin(pin).unwrap();
        assert_eq!((version.to_string(), sha), (engine_version, engine_sha.as_str()), "{pin}");
    }
}
