pub mod bundle;
pub mod locale;

pub use bundle::TranslationBundle;
pub use locale::Locale;

use std::sync::OnceLock;

static GLOBAL_BUNDLE: OnceLock<TranslationBundle> = OnceLock::new();

/// Initialize the global translation bundle for the given locale.
/// Call once at startup before any `t()` / `t_fmt()` calls.
///
/// Safe to call multiple times — only the first call takes effect.
pub fn init(locale: &Locale) {
    let _ = GLOBAL_BUNDLE.set(TranslationBundle::load(&locale.language));
}

/// Look up a translation key. Falls back to English, then to the raw key.
pub fn t(key: &str) -> &str {
    match GLOBAL_BUNDLE.get() {
        Some(b) => b.get(key),
        None => {
            let _ = GLOBAL_BUNDLE.set(TranslationBundle::load("en"));
            GLOBAL_BUNDLE.get().unwrap().get(key)
        }
    }
}

/// Look up a translation key with variable interpolation.
/// Replaces `{name}` placeholders with values from `args`.
pub fn t_fmt(key: &str, args: &[(&str, &str)]) -> String {
    match GLOBAL_BUNDLE.get() {
        Some(b) => b.get_fmt(key, args),
        None => {
            let _ = GLOBAL_BUNDLE.set(TranslationBundle::load("en"));
            GLOBAL_BUNDLE.get().unwrap().get_fmt(key, args)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn t_works_without_init() {
        assert_eq!(t("cmd.cube.label"), "Add Cube");
    }

    #[test]
    fn t_fmt_interpolates() {
        let result = t_fmt("status.parts", &[("count", "5")]);
        assert_eq!(result, "5 parts");
    }
}
