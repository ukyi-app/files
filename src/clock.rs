use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

/// 현재 UTC 시각을 RFC3339 문자열로.
pub fn now_rfc3339() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .expect("rfc3339 format")
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::format_description::well_known::Rfc3339;
    use time::OffsetDateTime;

    #[test]
    fn now_is_parseable_rfc3339() {
        let s = now_rfc3339();
        assert!(
            OffsetDateTime::parse(&s, &Rfc3339).is_ok(),
            "not RFC3339: {s}"
        );
    }
}
