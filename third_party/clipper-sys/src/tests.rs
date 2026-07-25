use super::{
    execute, free_polygons, offset, ClipType_ctDifference, ClipType_ctIntersection,
    ClipType_ctUnion, EndType_etClosedPolygon, JoinType_jtSquare, Path, PolyFillType_pftNonZero,
    PolyType_ptClip, PolyType_ptSubject, Polygon, Polygons,
};

#[test]
fn test_no_polygons() {
    let polygons = Polygons {
        polygons: std::ptr::null_mut(),
        polygons_count: 0,
    };
    let result = unsafe {
        execute(
            ClipType_ctUnion,
            polygons,
            PolyFillType_pftNonZero,
            PolyFillType_pftNonZero,
        )
    };
    unsafe { free_polygons(result) };
}

#[test]
fn test_intersection_with_hole() {
    let polygons = Polygons {
        polygons: [
            Polygon {
                type_: PolyType_ptSubject,
                paths: [
                    Path {
                        vertices: [[180, 200], [260, 200], [260, 150], [180, 150]].as_mut_ptr(),
                        vertices_count: 4,
                        closed: 1,
                    },
                    Path {
                        vertices: [[215, 160], [230, 190], [200, 190]].as_mut_ptr(),
                        vertices_count: 3,
                        closed: 1,
                    },
                ]
                .as_mut_ptr(),
                paths_count: 2,
            },
            Polygon {
                type_: PolyType_ptClip,
                paths: [Path {
                    vertices: [[190, 210], [240, 210], [240, 130], [190, 130]].as_mut_ptr(),
                    vertices_count: 4,
                    closed: 1,
                }]
                .as_mut_ptr(),
                paths_count: 1,
            },
        ]
        .as_mut_ptr(),
        polygons_count: 2,
    };

    let expected = Polygons {
        polygons: [Polygon {
            type_: PolyType_ptSubject,
            paths: [
                Path {
                    vertices: [[240, 200], [190, 200], [190, 150], [240, 150]].as_mut_ptr(),
                    vertices_count: 4,
                    closed: 1,
                },
                Path {
                    vertices: [[200, 190], [230, 190], [215, 160]].as_mut_ptr(),
                    vertices_count: 3,
                    closed: 1,
                },
            ]
            .as_mut_ptr(),
            paths_count: 2,
        }]
        .as_mut_ptr(),
        polygons_count: 1,
    };

    let result = unsafe {
        execute(
            ClipType_ctIntersection,
            polygons,
            PolyFillType_pftNonZero,
            PolyFillType_pftNonZero,
        )
    };

    assert_eq!(expected, result);
    unsafe { free_polygons(result) };
}

#[test]
fn test_difference() {
    let polygons = Polygons {
        polygons: [
            Polygon {
                type_: PolyType_ptSubject,
                paths: [Path {
                    vertices: [[180, 200], [260, 200], [260, 150], [180, 150]].as_mut_ptr(),
                    vertices_count: 4,
                    closed: 1,
                }]
                .as_mut_ptr(),
                paths_count: 1,
            },
            Polygon {
                type_: PolyType_ptClip,
                paths: [Path {
                    vertices: [[190, 210], [240, 210], [240, 130], [190, 130]].as_mut_ptr(),
                    vertices_count: 4,
                    closed: 1,
                }]
                .as_mut_ptr(),
                paths_count: 1,
            },
        ]
        .as_mut_ptr(),
        polygons_count: 2,
    };

    let expected = Polygons {
        polygons: [
            Polygon {
                type_: PolyType_ptSubject,
                paths: [Path {
                    vertices: [[190, 200], [180, 200], [180, 150], [190, 150]].as_mut_ptr(),
                    vertices_count: 4,
                    closed: 1,
                }]
                .as_mut_ptr(),
                paths_count: 1,
            },
            Polygon {
                type_: PolyType_ptSubject,
                paths: [Path {
                    vertices: [[260, 200], [240, 200], [240, 150], [260, 150]].as_mut_ptr(),
                    vertices_count: 4,
                    closed: 1,
                }]
                .as_mut_ptr(),
                paths_count: 1,
            },
        ]
        .as_mut_ptr(),
        polygons_count: 2,
    };

    let result = unsafe {
        execute(
            ClipType_ctDifference,
            polygons,
            PolyFillType_pftNonZero,
            PolyFillType_pftNonZero,
        )
    };

    assert_eq!(expected, result);
    unsafe { free_polygons(result) };
}

#[test]
fn test_offset() {
    let polygons = Polygons {
        polygons: [Polygon {
            type_: PolyType_ptSubject,
            paths: [Path {
                vertices: [[100, 100], [100, 200], [200, 200], [200, 100]].as_mut_ptr(),
                vertices_count: 4,
                closed: 1,
            }]
            .as_mut_ptr(),
            paths_count: 1,
        }]
        .as_mut_ptr(),
        polygons_count: 1,
    };

    let expected = Polygon {
        type_: PolyType_ptSubject,
        paths: [Path {
            vertices: [
                [205, 98],
                [205, 202],
                [202, 205],
                [98, 205],
                [95, 202],
                [95, 98],
                [98, 95],
                [202, 95],
            ]
            .as_mut_ptr(),
            vertices_count: 8,
            closed: 1,
        }]
        .as_mut_ptr(),
        paths_count: 1,
    };

    let (result, output) = unsafe {
        let result = offset(
            /*miter_limit=*/ 2.0,
            /*round_precision=*/ 0.25,
            JoinType_jtSquare,
            EndType_etClosedPolygon,
            polygons,
            5.,
        );

        let output = std::slice::from_raw_parts(result.polygons, 1)[0];

        (result, output)
    };

    let paths = unsafe { std::slice::from_raw_parts(output.paths, 1) };
    let vertices =
        unsafe { std::slice::from_raw_parts(paths[0].vertices, paths[0].vertices_count as usize) };

    assert_eq!(expected, output, "result vertices: {:?}", vertices);
    unsafe { free_polygons(result) };
}
