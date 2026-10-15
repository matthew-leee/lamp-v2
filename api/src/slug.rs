//! Product slug validation.

/// Returns `true` when `slug` is a valid product slug: 1-64 characters, all
/// lowercase ASCII letters, digits or hyphens.
///
/// Validity is defined purely by charset and length (per spec); leading or
/// trailing hyphens are accepted, e.g. `"-5"`.
pub fn validate_slug(slug: &str) -> bool {
    (1..=64).contains(&slug.chars().count())
        && slug
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_slugs() {
        assert!(validate_slug("ph-5"));
        assert!(validate_slug("artichoke-1958"));
        assert!(validate_slug("a"));
        assert!(validate_slug("5"));
        assert!(validate_slug("0-9"));
        assert!(validate_slug("-5")); // hyphens allowed anywhere per charset rule
        assert!(validate_slug(&"x".repeat(64))); // max length
    }

    #[test]
    fn empty_is_invalid() {
        assert!(!validate_slug(""));
    }

    #[test]
    fn too_long_is_invalid() {
        assert!(!validate_slug(&"x".repeat(65)));
    }

    #[test]
    fn bad_characters_are_invalid() {
        assert!(!validate_slug("ph_5")); // underscore
        assert!(!validate_slug("ph 5")); // space
        assert!(!validate_slug("ph.5")); // dot
        assert!(!validate_slug("ph+5"));
        assert!(!validate_slug("phé5")); // non-ASCII
        assert!(!validate_slug("PH-5")); // uppercase
        assert!(!validate_slug("Ph-5")); // mixed case
        assert!(!validate_slug("ph-5!"));
    }
}
