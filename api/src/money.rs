//! HKD money formatting. All money is integer cents — never floats.

/// Formats an integer cent amount as Hong Kong dollars, e.g. `161500` ->
/// `"HK$1,615.00"`.
///
/// # Panics
///
/// Panics on negative amounts. Prices are non-negative by schema
/// (`CHECK (price_cents >= 0)`), so a negative value here means an upstream bug
/// (e.g. a mis-signed discount) and is rejected loudly rather than rendered as a
/// plausible-looking price.
pub fn format_hkd(cents: i64) -> String {
    assert!(cents >= 0, "format_hkd: negative cent amount: {cents}");
    let dollars = cents / 100;
    let cents_part = cents % 100;
    format!("HK${}.{:02}", group_thousands(dollars), cents_part)
}

/// Groups a non-negative integer with commas every three digits: `1234567` ->
/// `"1,234,567"`.
fn group_thousands(n: i64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.char_indices() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero() {
        assert_eq!(format_hkd(0), "HK$0.00");
    }

    #[test]
    fn example_from_spec() {
        assert_eq!(format_hkd(161_500), "HK$1,615.00");
    }

    #[test]
    fn sub_dollar_and_dollar_boundaries() {
        assert_eq!(format_hkd(5), "HK$0.05");
        assert_eq!(format_hkd(99), "HK$0.99");
        assert_eq!(format_hkd(100), "HK$1.00");
        assert_eq!(format_hkd(105), "HK$1.05");
    }

    #[test]
    fn thousands_separators() {
        assert_eq!(format_hkd(1_000), "HK$10.00"); // below the grouping boundary
        assert_eq!(format_hkd(100_000), "HK$1,000.00");
        assert_eq!(format_hkd(100_000_000), "HK$1,000,000.00");
        assert_eq!(format_hkd(123_456_789), "HK$1,234,567.89");
        assert_eq!(format_hkd(999_999), "HK$9,999.99"); // exactly one group
    }

    #[test]
    #[should_panic(expected = "negative cent amount")]
    fn rejects_negative_one() {
        let _ = format_hkd(-1);
    }

    #[test]
    #[should_panic(expected = "negative cent amount")]
    fn rejects_negative_amount() {
        let _ = format_hkd(-161_500);
    }
}
