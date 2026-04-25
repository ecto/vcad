use std::collections::HashMap;

const EN_JSON: &str = include_str!("../locales/en.json");

/// A loaded set of translations for a single locale, with English fallback.
pub struct TranslationBundle {
    /// Current locale's strings (may be empty for English).
    locale: HashMap<String, String>,
    /// English strings — always populated as the fallback.
    english: HashMap<String, String>,
}

impl TranslationBundle {
    /// Load translations for the given language code.
    ///
    /// English is always embedded at compile time. Other locales are loaded
    /// from JSON files embedded via `include_str!` in [`load_locale`].
    pub fn load(language: &str) -> Self {
        let english: HashMap<String, String> =
            serde_json::from_str(EN_JSON).expect("en.json must be valid");

        let locale = if language == "en" {
            HashMap::new()
        } else {
            load_locale(language)
        };

        Self { locale, english }
    }

    /// Look up a translation key. Returns the locale-specific string if
    /// available, then English fallback, then the raw key as last resort.
    pub fn get<'a>(&'a self, key: &'a str) -> &'a str {
        if let Some(s) = self.locale.get(key) {
            return s;
        }
        if let Some(s) = self.english.get(key) {
            return s;
        }
        key
    }

    /// Look up with variable interpolation. Replaces `{name}` placeholders
    /// with the corresponding value from `args`.
    pub fn get_fmt(&self, key: &str, args: &[(&str, &str)]) -> String {
        let template = self.get(key);
        interpolate(template, args)
    }
}

/// Replace `{name}` placeholders in a template string.
fn interpolate(template: &str, args: &[(&str, &str)]) -> String {
    let mut result = template.to_string();
    for (name, value) in args {
        let placeholder = format!("{{{name}}}");
        result = result.replace(&placeholder, value);
    }
    result
}

/// Load a locale's JSON translations. Returns an empty map for unknown locales
/// (the bundle falls back to English gracefully).
fn load_locale(language: &str) -> HashMap<String, String> {
    let json = match language {
        "es" => Some(include_str!("../locales/es.json")),
        "fr" => Some(include_str!("../locales/fr.json")),
        _ => None,
    };
    match json {
        Some(s) => serde_json::from_str(s).unwrap_or_default(),
        None => HashMap::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn english_loads() {
        let b = TranslationBundle::load("en");
        assert_eq!(b.get("cmd.cube.label"), "Add Cube");
    }

    #[test]
    fn unknown_key_returns_key() {
        let b = TranslationBundle::load("en");
        assert_eq!(b.get("nonexistent.key"), "nonexistent.key");
    }

    #[test]
    fn interpolation() {
        let result = interpolate("{count} parts", &[("count", "3")]);
        assert_eq!(result, "3 parts");
    }

    #[test]
    fn interpolation_multiple() {
        let result = interpolate("{a} and {b}", &[("a", "X"), ("b", "Y")]);
        assert_eq!(result, "X and Y");
    }

    #[test]
    fn unknown_locale_falls_back() {
        let b = TranslationBundle::load("xx");
        assert_eq!(b.get("cmd.cube.label"), "Add Cube");
    }
}
