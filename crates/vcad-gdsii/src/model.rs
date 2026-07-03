//! Plain-data model of a GDSII library.
//!
//! Coordinates are `i32` database units, exactly as stored in the stream.
//! Multiply by [`Library::db_unit_in_meters`] to get physical dimensions.

/// A GDSII library: a named collection of cells sharing one unit system.
#[derive(Debug, Clone, PartialEq)]
pub struct Library {
    /// Library name (LIBNAME).
    pub name: String,
    /// Size of one database unit expressed in user units (first UNITS word).
    ///
    /// Typically `0.001`: user units are micrometers, database units are
    /// nanometers.
    pub user_unit: f64,
    /// Size of one database unit in meters (second UNITS word).
    ///
    /// Typically `1e-9` (nanometer database grid).
    pub db_unit_in_meters: f64,
    /// The cells (GDSII "structures") in definition order.
    pub cells: Vec<Cell>,
}

impl Library {
    /// Create an empty library with the conventional µm/nm unit system
    /// (`user_unit = 0.001`, `db_unit_in_meters = 1e-9`).
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            user_unit: 0.001,
            db_unit_in_meters: 1e-9,
            cells: Vec::new(),
        }
    }

    /// Look up a cell by name.
    pub fn cell(&self, name: &str) -> Option<&Cell> {
        self.cells.iter().find(|c| c.name == name)
    }
}

/// A GDSII cell (structure): a named list of elements.
#[derive(Debug, Clone, PartialEq)]
pub struct Cell {
    /// Structure name (STRNAME).
    pub name: String,
    /// Elements in stream order.
    pub elements: Vec<Element>,
}

impl Cell {
    /// Create an empty cell.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            elements: Vec::new(),
        }
    }
}

/// Reflection / magnification / rotation applied by SREF, AREF, and TEXT.
///
/// Applied to referenced geometry in this order: mirror about the X axis
/// (`y → −y`), scale by `mag`, rotate `angle_deg` counterclockwise, then
/// translate to the reference origin.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Strans {
    /// STRANS bit 0 (mask `0x8000`): reflect about the X axis before rotation.
    pub mirror_x: bool,
    /// Magnification factor (MAG record, default 1.0).
    pub mag: f64,
    /// Counterclockwise rotation in degrees (ANGLE record, default 0.0).
    pub angle_deg: f64,
}

impl Default for Strans {
    fn default() -> Self {
        Self {
            mirror_x: false,
            mag: 1.0,
            angle_deg: 0.0,
        }
    }
}

impl Strans {
    /// True if this transform is the identity (no records need writing).
    pub fn is_identity(&self) -> bool {
        !self.mirror_x && self.mag == 1.0 && self.angle_deg == 0.0
    }
}

/// A single layout element inside a cell.
#[derive(Debug, Clone, PartialEq)]
pub enum Element {
    /// A filled polygon (BOUNDARY). By GDSII convention the last point
    /// repeats the first; the reader preserves whatever the stream stored.
    Boundary {
        /// GDS layer number.
        layer: i16,
        /// GDS datatype number.
        datatype: i16,
        /// Vertices in database units.
        xy: Vec<(i32, i32)>,
    },
    /// A wire with width (PATH), drawn along a centerline.
    Path {
        /// GDS layer number.
        layer: i16,
        /// GDS datatype number.
        datatype: i16,
        /// End style: 0 = flush, 1 = round, 2 = half-width extension.
        pathtype: i16,
        /// Width in database units (0 if absent).
        width: i32,
        /// Centerline vertices in database units.
        xy: Vec<(i32, i32)>,
    },
    /// Annotation text (TEXT). Not geometry — ignored by the flattener.
    Text {
        /// GDS layer number.
        layer: i16,
        /// GDS text type number.
        texttype: i16,
        /// Anchor position in database units.
        origin: (i32, i32),
        /// Transform applied to the text.
        strans: Strans,
        /// The text string.
        string: String,
    },
    /// A reference to another cell (SREF).
    Sref {
        /// Name of the referenced cell.
        sname: String,
        /// Transform applied to the referenced cell.
        strans: Strans,
        /// Placement origin in database units.
        origin: (i32, i32),
    },
    /// An array of references to another cell (AREF).
    Aref {
        /// Name of the referenced cell.
        sname: String,
        /// Transform applied to each array instance.
        strans: Strans,
        /// Number of columns.
        cols: i16,
        /// Number of rows.
        rows: i16,
        /// Three lattice points: array origin, the point
        /// `origin + cols·column_step`, and the point `origin + rows·row_step`.
        xy: [(i32, i32); 3],
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_strans_is_identity() {
        assert!(Strans::default().is_identity());
        assert!(!Strans {
            mirror_x: true,
            ..Strans::default()
        }
        .is_identity());
        assert!(!Strans {
            mag: 2.0,
            ..Strans::default()
        }
        .is_identity());
    }

    #[test]
    fn library_cell_lookup() {
        let mut lib = Library::new("top");
        lib.cells.push(Cell::new("a"));
        lib.cells.push(Cell::new("b"));
        assert_eq!(lib.cell("b").unwrap().name, "b");
        assert!(lib.cell("missing").is_none());
        assert_eq!(lib.user_unit, 0.001);
        assert_eq!(lib.db_unit_in_meters, 1e-9);
    }
}
