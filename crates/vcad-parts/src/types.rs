//! Types shared by every built-in part.

use serde::Serialize;
use std::collections::HashMap;
use vcad_ir::Document;

/// Function signature that every built-in part's `build` function implements.
pub type BuildFn = fn(&Params) -> Result<Document, String>;

/// A single registered part: its metadata plus the build closure.
pub struct PartEntry {
    /// Static metadata (name, category, params, xrefs, …).
    pub meta: PartMetadata,
    /// Compiled builder function.
    pub build: BuildFn,
}

impl PartEntry {
    /// Produce a JSON-serializable manifest entry (consumed by the app).
    pub fn manifest_entry(&self) -> ManifestEntry {
        ManifestEntry {
            id: self.meta.id.to_string(),
            name: self.meta.name.to_string(),
            category: self.meta.category.to_string(),
            description: self.meta.description.map(|s| s.to_string()),
            version: self.meta.version.to_string(),
            synonyms: self.meta.synonyms.iter().map(|s| s.to_string()).collect(),
            params: self.meta.params.iter().map(ParamManifest::from).collect(),
            xrefs: self.meta.xrefs.iter().map(XrefManifest::from).collect(),
            thumb_svg: std::str::from_utf8(self.meta.thumb)
                .ok()
                .map(|s| s.to_string()),
            search_tokens: self.search_tokens(),
        }
    }

    fn search_tokens(&self) -> Vec<String> {
        let mut tokens: Vec<String> = Vec::new();
        tokens.push(self.meta.id.to_string());
        tokens.push(self.meta.name.to_string());
        tokens.push(self.meta.category.to_string());
        for s in self.meta.synonyms {
            tokens.push(s.to_string());
        }
        for x in self.meta.xrefs {
            if let Some(m) = x.mcmaster {
                tokens.push(m.to_string());
            }
            if let Some(v) = x.iso {
                tokens.push(v.to_string());
            }
            if let Some(v) = x.din {
                tokens.push(v.to_string());
            }
        }
        tokens
    }
}

/// Compile-time metadata for a built-in part.
pub struct PartMetadata {
    /// Dotted identifier, e.g. `"fastener.bolt.socket-head"`. No `std:` prefix.
    pub id: &'static str,
    /// Human-readable name displayed in palette + Cmd+K.
    pub name: &'static str,
    /// Category label ("Fasteners", "Bearings", …). Drives palette grouping.
    pub category: &'static str,
    /// Optional short description for tooltips.
    pub description: Option<&'static str>,
    /// Declared parameters with types and defaults.
    pub params: &'static [Param],
    /// Cross-references to external catalogs (McMaster, ISO, DIN, …).
    pub xrefs: &'static [Xref],
    /// Alternate names used for fuzzy search in Cmd+K.
    pub synonyms: &'static [&'static str],
    /// Semver version string.
    pub version: &'static str,
    /// SVG thumbnail bytes, embedded at compile time via `include_bytes!`.
    pub thumb: &'static [u8],
}

/// A single parameter declaration on a part.
pub enum Param {
    /// A physical length with unit (mm by default).
    Length {
        /// Parameter name used in `params` map keys.
        name: &'static str,
        /// Minimum allowed value (inclusive).
        min: f64,
        /// Maximum allowed value (inclusive).
        max: f64,
        /// Default value when unspecified.
        default: f64,
        /// Display unit label.
        unit: &'static str,
    },
    /// Unitless floating-point number.
    Number {
        /// Parameter name.
        name: &'static str,
        /// Minimum allowed value.
        min: f64,
        /// Maximum allowed value.
        max: f64,
        /// Default value.
        default: f64,
    },
    /// Integer parameter.
    Integer {
        /// Parameter name.
        name: &'static str,
        /// Minimum allowed value.
        min: i64,
        /// Maximum allowed value.
        max: i64,
        /// Default value.
        default: i64,
    },
    /// One-of-enum chosen from a static list.
    Enum {
        /// Parameter name.
        name: &'static str,
        /// Allowed values.
        values: &'static [&'static str],
        /// Default selection.
        default: &'static str,
    },
    /// Boolean flag.
    Boolean {
        /// Parameter name.
        name: &'static str,
        /// Default value.
        default: bool,
    },
}

impl Param {
    /// Parameter name, regardless of variant.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Length { name, .. }
            | Self::Number { name, .. }
            | Self::Integer { name, .. }
            | Self::Enum { name, .. }
            | Self::Boolean { name, .. } => name,
        }
    }

    /// Default value as JSON.
    pub fn default_json(&self) -> serde_json::Value {
        match self {
            Self::Length { default, .. } | Self::Number { default, .. } => {
                serde_json::json!(default)
            }
            Self::Integer { default, .. } => serde_json::json!(default),
            Self::Enum { default, .. } => serde_json::json!(default),
            Self::Boolean { default, .. } => serde_json::json!(default),
        }
    }
}

/// A catalog alias row linking a specific parameter combination to external
/// part numbers (McMaster, ISO, DIN, ...).
pub struct Xref {
    /// Parameter key/value pairs this xref applies to, e.g. `&[("size","M6"),("length","20")]`.
    pub params: &'static [(&'static str, &'static str)],
    /// McMaster-Carr part number, if any.
    pub mcmaster: Option<&'static str>,
    /// ISO standard reference, if any.
    pub iso: Option<&'static str>,
    /// DIN standard reference, if any.
    pub din: Option<&'static str>,
}

/// Runtime parameter accessor passed to `build` functions.
///
/// Wraps the incoming `params` map and applies declared defaults from the
/// part's [`PartMetadata`] when a value is missing.
pub struct Params<'a> {
    declared: &'a [Param],
    supplied: &'a HashMap<String, serde_json::Value>,
}

impl<'a> Params<'a> {
    /// Wrap supplied params against the part's declared parameter list.
    pub fn new(declared: &'a [Param], supplied: &'a HashMap<String, serde_json::Value>) -> Self {
        Self { declared, supplied }
    }

    fn raw(&self, key: &str) -> Option<&serde_json::Value> {
        self.supplied.get(key)
    }

    fn declared_default(&self, key: &str) -> Option<serde_json::Value> {
        self.declared
            .iter()
            .find(|p| p.name() == key)
            .map(|p| p.default_json())
    }

    /// Read a floating-point parameter, falling back to the declared default.
    pub fn f64(&self, key: &str) -> f64 {
        let v = self
            .raw(key)
            .cloned()
            .or_else(|| self.declared_default(key))
            .unwrap_or(serde_json::Value::Null);
        match v {
            serde_json::Value::Number(n) => n.as_f64().unwrap_or(0.0),
            serde_json::Value::String(s) => s.parse().unwrap_or(0.0),
            _ => 0.0,
        }
    }

    /// Read an integer parameter.
    pub fn i64(&self, key: &str) -> i64 {
        let v = self
            .raw(key)
            .cloned()
            .or_else(|| self.declared_default(key))
            .unwrap_or(serde_json::Value::Null);
        match v {
            serde_json::Value::Number(n) => n.as_i64().unwrap_or(0),
            serde_json::Value::String(s) => s.parse().unwrap_or(0),
            _ => 0,
        }
    }

    /// Read a string parameter (including enum values).
    pub fn str(&self, key: &str) -> String {
        let v = self
            .raw(key)
            .cloned()
            .or_else(|| self.declared_default(key))
            .unwrap_or(serde_json::Value::Null);
        match v {
            serde_json::Value::String(s) => s,
            serde_json::Value::Number(n) => n.to_string(),
            serde_json::Value::Bool(b) => b.to_string(),
            _ => String::new(),
        }
    }

    /// Read a boolean parameter.
    pub fn bool(&self, key: &str) -> bool {
        let v = self
            .raw(key)
            .cloned()
            .or_else(|| self.declared_default(key))
            .unwrap_or(serde_json::Value::Null);
        match v {
            serde_json::Value::Bool(b) => b,
            serde_json::Value::String(s) => s == "true",
            _ => false,
        }
    }
}

/// JSON-serializable manifest entry. Shape is consumed by the TypeScript app.
#[derive(Serialize)]
pub struct ManifestEntry {
    /// Dotted part identifier (no `std:` prefix).
    pub id: String,
    /// Display name.
    pub name: String,
    /// Category label.
    pub category: String,
    /// Optional description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Semver version string.
    pub version: String,
    /// Synonym tokens for fuzzy search.
    pub synonyms: Vec<String>,
    /// Parameter declarations.
    pub params: Vec<ParamManifest>,
    /// Catalog xrefs.
    pub xrefs: Vec<XrefManifest>,
    /// Inline SVG for the palette thumbnail (if UTF-8).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumb_svg: Option<String>,
    /// Union of search tokens (name, category, synonyms, xref numbers).
    pub search_tokens: Vec<String>,
}

/// JSON-serializable parameter declaration.
#[derive(Serialize)]
#[serde(tag = "kind")]
pub enum ParamManifest {
    /// Length with unit.
    #[serde(rename = "length")]
    Length {
        /// Parameter name.
        name: String,
        /// Minimum value.
        min: f64,
        /// Maximum value.
        max: f64,
        /// Default value.
        default: f64,
        /// Display unit.
        unit: String,
    },
    /// Unitless number.
    #[serde(rename = "number")]
    Number {
        /// Parameter name.
        name: String,
        /// Minimum value.
        min: f64,
        /// Maximum value.
        max: f64,
        /// Default value.
        default: f64,
    },
    /// Integer.
    #[serde(rename = "integer")]
    Integer {
        /// Parameter name.
        name: String,
        /// Minimum value.
        min: i64,
        /// Maximum value.
        max: i64,
        /// Default value.
        default: i64,
    },
    /// Enum.
    #[serde(rename = "enum")]
    Enum {
        /// Parameter name.
        name: String,
        /// Allowed values.
        values: Vec<String>,
        /// Default selection.
        default: String,
    },
    /// Boolean.
    #[serde(rename = "boolean")]
    Boolean {
        /// Parameter name.
        name: String,
        /// Default value.
        default: bool,
    },
}

impl From<&Param> for ParamManifest {
    fn from(p: &Param) -> Self {
        match p {
            Param::Length {
                name,
                min,
                max,
                default,
                unit,
            } => ParamManifest::Length {
                name: name.to_string(),
                min: *min,
                max: *max,
                default: *default,
                unit: unit.to_string(),
            },
            Param::Number {
                name,
                min,
                max,
                default,
            } => ParamManifest::Number {
                name: name.to_string(),
                min: *min,
                max: *max,
                default: *default,
            },
            Param::Integer {
                name,
                min,
                max,
                default,
            } => ParamManifest::Integer {
                name: name.to_string(),
                min: *min,
                max: *max,
                default: *default,
            },
            Param::Enum {
                name,
                values,
                default,
            } => ParamManifest::Enum {
                name: name.to_string(),
                values: values.iter().map(|s| s.to_string()).collect(),
                default: default.to_string(),
            },
            Param::Boolean { name, default } => ParamManifest::Boolean {
                name: name.to_string(),
                default: *default,
            },
        }
    }
}

/// JSON-serializable xref entry.
#[derive(Serialize)]
pub struct XrefManifest {
    /// Matching parameters.
    pub params: HashMap<String, String>,
    /// McMaster part number.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mcmaster: Option<String>,
    /// ISO reference.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub iso: Option<String>,
    /// DIN reference.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub din: Option<String>,
}

impl From<&Xref> for XrefManifest {
    fn from(x: &Xref) -> Self {
        XrefManifest {
            params: x
                .params
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            mcmaster: x.mcmaster.map(|s| s.to_string()),
            iso: x.iso.map(|s| s.to_string()),
            din: x.din.map(|s| s.to_string()),
        }
    }
}
