use redos_detector::{is_safe, Config, Error};

fn assert_parse_error(source: &str, flags: &str) {
    let error = is_safe(source, flags, &Config::default()).unwrap_err();
    assert!(
        matches!(error, Error::Parse(_)),
        "{source:?} with flags {flags:?} returned {error:?}"
    );
}

#[test]
fn unicode_decimal_escape_without_capture_is_parse_error() {
    for source in [r"^\2$", r"^\8$"] {
        assert_parse_error(source, "u");
    }
}

#[test]
fn reversed_character_class_range_is_parse_error() {
    for flags in ["", "u"] {
        assert_parse_error(r"^[z-a]$", flags);
    }
}
