use crate::{Path, PolyType, Polygon, Polygons, Vertice};

pub struct PolygonsDebug<'a>(pub &'a Polygons);
pub struct PolygonDebug<'a>(pub &'a Polygon);
pub struct PathDebug<'a>(pub &'a Path);
pub struct PathsDebug<'a>(pub &'a [Path]);
pub struct PolyTypeDebug<'a>(pub &'a PolyType);
pub struct VerticesDebug<'a>(pub &'a [Vertice]);
pub struct VerticeDebug<'a>(pub &'a Vertice);

impl<'a> std::fmt::Debug for PolygonsDebug<'a> {
    fn fmt(&self, fmt: &mut std::fmt::Formatter) -> std::fmt::Result {
        fmt.debug_list()
            .entries(
                self.0
                    .polygons()
                    .iter()
                    .map(|polygon| PolygonDebug(polygon)),
            )
            .finish()
    }
}

impl<'a> std::fmt::Debug for PolygonDebug<'a> {
    fn fmt(&self, fmt: &mut std::fmt::Formatter) -> std::fmt::Result {
        fmt.debug_struct("Polygon")
            .field("paths", &PathsDebug(self.0.paths()))
            .field("type", &PolyTypeDebug(&self.0.type_))
            .finish()
    }
}

impl<'a> std::fmt::Debug for PathDebug<'a> {
    fn fmt(&self, fmt: &mut std::fmt::Formatter) -> std::fmt::Result {
        fmt.debug_struct("Path")
            .field("vertices", &VerticesDebug(self.0.vertices()))
            .field("closed", &self.0.closed)
            .finish()
    }
}

impl<'a> std::fmt::Debug for VerticesDebug<'a> {
    fn fmt(&self, fmt: &mut std::fmt::Formatter) -> std::fmt::Result {
        fmt.debug_list()
            .entries(self.0.iter().map(|vertice| VerticeDebug(vertice)))
            .finish()
    }
}

impl<'a> std::fmt::Debug for VerticeDebug<'a> {
    fn fmt(&self, fmt: &mut std::fmt::Formatter) -> std::fmt::Result {
        fmt.debug_list()
            .entry(&self.0[0])
            .entry(&self.0[1])
            .finish()
    }
}

impl<'a> std::fmt::Debug for PathsDebug<'a> {
    fn fmt(&self, fmt: &mut std::fmt::Formatter) -> std::fmt::Result {
        fmt.debug_list()
            .entries(self.0.iter().map(|path| PathDebug(path)))
            .finish()
    }
}

impl<'a> std::fmt::Debug for PolyTypeDebug<'a> {
    fn fmt(&self, fmt: &mut std::fmt::Formatter) -> std::fmt::Result {
        fmt.write_str(match *self.0 {
            crate::clipper::PolyType_ptSubject => "ptSubject",
            crate::clipper::PolyType_ptClip => "ptClip",
            _ => "invalid type",
        })
    }
}
