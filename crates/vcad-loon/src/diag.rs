//! Source locations for loon evaluation errors.
//!
//! The program the interpreter sees is not the file the author wrote: the
//! vcad library is prepended, and a source with several top-level values is
//! rewritten into `let` bindings (see `collect_top_level_values`). A parser
//! or interpreter span therefore points into that assembled text. This
//! module keeps the map from assembled offsets back to the author's source,
//! records the span of the most recent failure on this thread, and turns it
//! into `file:line:column`.
//!
//! An error raised inside an imported module carries a span into *that*
//! module's text, which this map cannot tell apart from a root-program span.
//! So a location is only reported when the failing span is attributable to
//! the root program: parse errors always are; runtime errors are when the
//! interpreter's innermost call site (or the error span itself, with no
//! call stack) maps into the author's source.

use std::cell::RefCell;
use std::fmt;
use std::path::{Path, PathBuf};

/// A contiguous run of the author's source copied into the assembled text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Segment {
    /// Offset in the rewritten user source.
    pub out_start: usize,
    /// Offset in the author's source.
    pub src_start: usize,
    /// Length in bytes.
    pub len: usize,
}

/// Assembled-text offsets → author-source offsets.
#[derive(Debug, Clone, Default)]
pub(crate) struct SourceMap {
    /// Bytes before the user source (library + separator).
    pub prefix_len: usize,
    pub segments: Vec<Segment>,
}

impl SourceMap {
    /// The author-source offset of an assembled offset, if it falls in text
    /// the author wrote.
    pub fn to_source(&self, assembled: usize) -> Option<usize> {
        let u = assembled.checked_sub(self.prefix_len)?;
        self.segments
            .iter()
            .find(|s| u >= s.out_start && u < s.out_start + s.len.max(1))
            .map(|s| s.src_start + (u - s.out_start))
    }
}

thread_local! {
    static MAP: RefCell<Option<SourceMap>> = const { RefCell::new(None) };
    static LAST: RefCell<Option<usize>> = const { RefCell::new(None) };
}

pub(crate) fn set_map(map: SourceMap) {
    MAP.with(|m| *m.borrow_mut() = Some(map));
}

/// Forget the last failure location (called as each phase starts).
pub(crate) fn clear_failure() {
    LAST.with(|l| *l.borrow_mut() = None);
}

/// Record a failure at the given assembled-text offsets, innermost first;
/// the first that maps into the author's source wins.
pub(crate) fn record_failure(candidates: impl IntoIterator<Item = usize>) {
    let at = MAP.with(|m| {
        let m = m.borrow();
        let m = m.as_ref()?;
        candidates.into_iter().find_map(|c| m.to_source(c))
    });
    LAST.with(|l| *l.borrow_mut() = at);
}

fn take_failure() -> Option<usize> {
    LAST.with(|l| l.borrow_mut().take())
}

/// A loon evaluation error with its location in the author's file, when one
/// can be attributed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoonDiagnostic {
    /// The error message.
    pub message: String,
    /// The file the location refers to.
    pub file: Option<PathBuf>,
    /// 1-based line.
    pub line: Option<u32>,
    /// 1-based column (in characters).
    pub column: Option<u32>,
}

impl fmt::Display for LoonDiagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (&self.file, self.line, self.column) {
            (Some(p), Some(l), Some(c)) => write!(f, "{}:{l}:{c}: {}", p.display(), self.message),
            (Some(p), Some(l), None) => write!(f, "{}:{l}: {}", p.display(), self.message),
            _ => f.write_str(&self.message),
        }
    }
}

/// 1-based (line, column) of a byte offset in `text`.
pub fn line_col(text: &str, offset: usize) -> (u32, u32) {
    let offset = offset.min(text.len());
    let mut off = offset;
    while !text.is_char_boundary(off) {
        off -= 1;
    }
    let before = &text[..off];
    let line = before.matches('\n').count() as u32 + 1;
    let col = before.rsplit('\n').next().map_or(0, |l| l.chars().count()) as u32 + 1;
    (line, col)
}

/// Evaluate a loon file, reporting a failure with its source location.
///
/// Same evaluation as [`crate::eval_vcad_file`]: `[use ...]` resolves against
/// the file's directory, then `$VCAD_LOON_PATH`.
pub fn eval_vcad_file_diagnostic(path: &Path) -> Result<vcad_ir::Document, LoonDiagnostic> {
    let text = std::fs::read_to_string(path).map_err(|e| LoonDiagnostic {
        message: format!("cannot read {}: {e}", path.display()),
        file: Some(path.to_path_buf()),
        line: None,
        column: None,
    })?;
    let lead = text.len() - text.trim_start().len();
    clear_failure();
    crate::eval_vcad(text.trim(), path.parent()).map_err(|message| {
        let (line, column) = match take_failure() {
            Some(off) => {
                let (l, c) = line_col(&text, lead + off);
                (Some(l), Some(c))
            }
            None => (None, None),
        };
        LoonDiagnostic {
            message,
            file: Some(path.to_path_buf()),
            line,
            column,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str, src: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("vcad-loon-diag-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(format!("{name}.loon"));
        std::fs::write(&p, src).unwrap();
        p
    }

    #[test]
    fn line_col_counts_from_one() {
        assert_eq!(line_col("ab\ncd", 0), (1, 1));
        assert_eq!(line_col("ab\ncd", 4), (2, 2));
    }

    #[test]
    fn good_file_evaluates() {
        let p = tmp("good", "[root [cube 10 10 10] \"steel\"]\n");
        assert!(eval_vcad_file_diagnostic(&p).is_ok());
    }

    #[test]
    fn parse_error_points_at_its_line() {
        // Unclosed bracket on line 3.
        let p = tmp(
            "parse",
            "; a part\n[let w 10]\n[root [cube w w w \"steel\"\n",
        );
        let d = eval_vcad_file_diagnostic(&p).unwrap_err();
        assert_eq!(d.file.as_deref(), Some(p.as_path()));
        assert!(d.line.is_some(), "{d}");
        assert!(d.line.unwrap() >= 3, "{d}");
    }

    #[test]
    fn runtime_error_points_at_its_line() {
        let p = tmp(
            "runtime",
            "[let w 10]\n\n[root [cube w nope w] \"steel\"]\n",
        );
        let d = eval_vcad_file_diagnostic(&p).unwrap_err();
        assert!(d.message.contains("nope"), "{d}");
        assert_eq!(d.line, Some(3), "{d}");
    }

    #[test]
    fn multi_value_rewrite_maps_back() {
        // Two top-level values: the source is rewritten into lets, so the
        // map must carry the second form's offset back to line 4.
        let p = tmp(
            "multi",
            "[root [cube 1 1 1] \"steel\"]\n\n\n[root [cube 1 bogus 1] \"steel\"]\n",
        );
        let d = eval_vcad_file_diagnostic(&p).unwrap_err();
        assert!(d.message.contains("bogus"), "{d}");
        assert_eq!(d.line, Some(4), "{d}");
    }
}
