//! Thread color definitions and palettes.

use serde::{Deserialize, Serialize};

/// A thread color used in an embroidery pattern.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Thread {
    /// RGB color.
    pub color: [u8; 3],
    /// Display name (e.g. "Brother #007 - White").
    pub name: String,
    /// Thread brand (e.g. "Brother").
    pub brand: Option<String>,
    /// Catalog/part number.
    pub catalog_number: Option<String>,
}

impl Thread {
    /// Create a thread with just a color and name.
    pub fn new(color: [u8; 3], name: impl Into<String>) -> Self {
        Self {
            color,
            name: name.into(),
            brand: None,
            catalog_number: None,
        }
    }
}

/// The standard Brother 64-color PEC thread palette.
///
/// Index 0 is unused (reserved). Indices 1–64 map to the standard Brother colors.
pub static BROTHER_PALETTE: &[Thread] = &[];

// Populated at module level via lazy initialization — see `brother_palette()`.

/// Returns the full 65-entry Brother PEC thread palette (index 0 = placeholder).
pub fn brother_palette() -> Vec<Thread> {
    // Source: Brother PE-Design color chart / libembroidery
    let raw: &[(u8, u8, u8, &str)] = &[
        (0, 0, 0, "Unknown"),                 // 0: placeholder
        (14, 31, 124, "Prussian Blue"),       // 1
        (10, 85, 163, "Blue"),                // 2
        (48, 135, 119, "Teal Green"),         // 3
        (75, 107, 175, "Cornflower Blue"),    // 4
        (237, 23, 31, "Red"),                 // 5
        (209, 92, 0, "Reddish Brown"),        // 6
        (145, 54, 151, "Magenta"),            // 7
        (228, 154, 203, "Light Lilac"),       // 8
        (145, 95, 172, "Lilac"),              // 9
        (158, 214, 125, "Mint Green"),        // 10
        (232, 169, 0, "Deep Gold"),           // 11
        (254, 186, 53, "Orange"),             // 12
        (255, 255, 0, "Yellow"),              // 13
        (112, 188, 31, "Lime Green"),         // 14
        (186, 152, 0, "Brass"),               // 15
        (168, 168, 168, "Silver"),            // 16
        (125, 111, 0, "Russet Brown"),        // 17
        (255, 255, 179, "Cream Brown"),       // 18
        (79, 85, 86, "Pewter"),               // 19
        (0, 0, 0, "Black"),                   // 20
        (11, 61, 145, "Ultramarine"),         // 21
        (119, 1, 118, "Royal Purple"),        // 22
        (41, 49, 51, "Dark Gray"),            // 23
        (42, 19, 1, "Dark Brown"),            // 24
        (246, 74, 138, "Deep Rose"),          // 25
        (178, 118, 36, "Light Brown"),        // 26
        (252, 187, 197, "Salmon Pink"),       // 27
        (254, 55, 15, "Vermillion"),          // 28
        (240, 240, 240, "White"),             // 29
        (106, 28, 138, "Violet"),             // 30
        (168, 221, 196, "Seacrest"),          // 31
        (37, 132, 187, "Sky Blue"),           // 32
        (254, 179, 67, "Pumpkin"),            // 33
        (255, 243, 107, "Cream Yellow"),      // 34
        (208, 166, 96, "Khaki"),              // 35
        (209, 84, 0, "Clay Brown"),           // 36
        (102, 186, 73, "Leaf Green"),         // 37
        (19, 74, 70, "Peacock Blue"),         // 38
        (135, 135, 135, "Gray"),              // 39
        (216, 204, 198, "Warm Gray"),         // 40
        (67, 86, 7, "Dark Olive"),            // 41
        (253, 217, 222, "Flesh Pink"),        // 42
        (249, 147, 188, "Pink"),              // 43
        (0, 56, 34, "Deep Green"),            // 44
        (178, 175, 212, "Lavender"),          // 45
        (104, 106, 176, "Wisteria Blue"),     // 46
        (239, 227, 185, "Beige"),             // 47
        (247, 56, 102, "Carmine"),            // 48
        (181, 75, 100, "Amber Red"),          // 49
        (19, 43, 26, "Olive Green"),          // 50
        (199, 1, 86, "Dark Fuchsia"),         // 51
        (254, 158, 50, "Tangerine"),          // 52
        (168, 222, 235, "Light Blue"),        // 53
        (0, 103, 62, "Emerald Green"),        // 54
        (78, 41, 144, "Purple"),              // 55
        (47, 126, 32, "Moss Green"),          // 56
        (255, 204, 204, "Flesh Pink 2"),      // 57  (unofficial name)
        (255, 217, 17, "Harvest Gold"),       // 58
        (9, 91, 166, "Electric Blue"),        // 59
        (240, 249, 112, "Lemon Yellow"),      // 60
        (227, 243, 91, "Fresh Green"),        // 61
        (255, 153, 0, "Applique Material"),   // 62
        (255, 240, 141, "Applique Position"), // 63
        (255, 200, 200, "Applique"),          // 64
    ];

    raw.iter()
        .enumerate()
        .map(|(i, &(r, g, b, name))| Thread {
            color: [r, g, b],
            name: format!("Brother #{:03} - {}", i, name),
            brand: Some("Brother".into()),
            catalog_number: Some(format!("{}", i)),
        })
        .collect()
}
