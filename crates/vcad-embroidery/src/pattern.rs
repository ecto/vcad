//! The core embroidery pattern type.

use crate::error::{EmbroideryError, Result};
use crate::hoop::Hoop;
use crate::stats::{compute_stats, PatternStats};
use crate::stitch::StitchGroup;
use crate::thread::Thread;
use serde::{Deserialize, Serialize};

/// Metadata about an embroidery pattern.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PatternMetadata {
    /// Pattern name / title.
    pub name: String,
    /// Author or designer.
    pub author: String,
    /// Category (e.g. "floral", "lettering").
    pub category: Option<String>,
}

/// A complete embroidery pattern.
///
/// This is the format-agnostic representation used by all embroidery operations.
/// Coordinates are in millimeters, with the origin at the pattern center.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EmbPattern {
    /// Thread palette (ordered). Stitch groups reference threads by index.
    pub threads: Vec<Thread>,
    /// Groups of stitches, each associated with a thread color.
    pub stitch_groups: Vec<StitchGroup>,
    /// Pattern metadata.
    pub metadata: PatternMetadata,
}

impl EmbPattern {
    /// Create an empty pattern.
    pub fn new() -> Self {
        Self {
            threads: Vec::new(),
            stitch_groups: Vec::new(),
            metadata: PatternMetadata::default(),
        }
    }

    /// Compute pattern statistics.
    pub fn stats(&self) -> PatternStats {
        compute_stats(self)
    }

    /// Validate the pattern for internal consistency.
    pub fn validate(&self) -> Result<()> {
        if self.stitch_groups.is_empty() {
            return Err(EmbroideryError::EmptyPattern);
        }
        for (i, group) in self.stitch_groups.iter().enumerate() {
            if group.commands.is_empty() {
                return Err(EmbroideryError::EmptyStitchGroup(i));
            }
            if group.thread_index >= self.threads.len() {
                return Err(EmbroideryError::InvalidThreadIndex {
                    index: group.thread_index,
                    count: self.threads.len(),
                });
            }
        }
        Ok(())
    }

    /// Check whether the pattern fits within a hoop.
    pub fn fits_hoop(&self, hoop: &Hoop) -> bool {
        let stats = self.stats();
        hoop.contains((
            stats.bounds_min.0,
            stats.bounds_min.1,
            stats.bounds_max.0,
            stats.bounds_max.1,
        ))
    }

    /// Total stitch count across all groups.
    pub fn total_stitch_count(&self) -> usize {
        self.stitch_groups.iter().map(|g| g.stitch_count()).sum()
    }
}

impl Default for EmbPattern {
    fn default() -> Self {
        Self::new()
    }
}
