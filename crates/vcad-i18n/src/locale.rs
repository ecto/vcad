/// A locale identifier parsed from a BCP 47 tag or POSIX locale string.
#[derive(Debug, Clone)]
pub struct Locale {
    pub language: String,
    pub region: Option<String>,
}

impl Locale {
    /// Detect locale from environment variables (`LANG`, `LC_MESSAGES`, `LC_ALL`).
    ///
    /// Falls back to `"en"` if nothing is set or the value is `"C"` / `"POSIX"`.
    pub fn from_env() -> Self {
        let raw = std::env::var("LC_ALL")
            .or_else(|_| std::env::var("LC_MESSAGES"))
            .or_else(|_| std::env::var("LANG"))
            .unwrap_or_default();
        Self::parse(&raw)
    }

    /// Parse a BCP 47 tag (`"en-US"`, `"es"`) or POSIX locale (`"es_MX.UTF-8"`).
    pub fn parse(tag: &str) -> Self {
        let tag = tag.trim();
        if tag.is_empty() || tag == "C" || tag == "POSIX" {
            return Self {
                language: "en".into(),
                region: None,
            };
        }

        // Strip encoding suffix (e.g. ".UTF-8")
        let base = tag.split('.').next().unwrap_or(tag);

        // Split on '-' (BCP 47) or '_' (POSIX)
        let mut parts = base.split(['-', '_']);
        let language = parts
            .next()
            .unwrap_or("en")
            .to_ascii_lowercase();
        let region = parts.next().map(|r| r.to_ascii_uppercase());

        Self { language, region }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_bcp47() {
        let l = Locale::parse("en-US");
        assert_eq!(l.language, "en");
        assert_eq!(l.region.as_deref(), Some("US"));
    }

    #[test]
    fn parse_posix() {
        let l = Locale::parse("es_MX.UTF-8");
        assert_eq!(l.language, "es");
        assert_eq!(l.region.as_deref(), Some("MX"));
    }

    #[test]
    fn parse_bare() {
        let l = Locale::parse("de");
        assert_eq!(l.language, "de");
        assert!(l.region.is_none());
    }

    #[test]
    fn parse_empty_falls_back() {
        let l = Locale::parse("");
        assert_eq!(l.language, "en");
    }

    #[test]
    fn parse_c_falls_back() {
        let l = Locale::parse("C");
        assert_eq!(l.language, "en");
    }
}
