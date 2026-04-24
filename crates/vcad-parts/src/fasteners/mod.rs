//! Fastener parts — bolts, nuts, washers, screws.

pub mod bolt_socket_head;
pub mod nut_hex;
pub mod washer_flat;

/// ISO metric size → major diameter (mm).
pub fn metric_major_dia(size: &str) -> f64 {
    match size {
        "M2" => 2.0,
        "M2.5" => 2.5,
        "M3" => 3.0,
        "M4" => 4.0,
        "M5" => 5.0,
        "M6" => 6.0,
        "M8" => 8.0,
        "M10" => 10.0,
        "M12" => 12.0,
        "M16" => 16.0,
        "M20" => 20.0,
        _ => 6.0,
    }
}

/// ISO metric size → ISO 7089 flat washer outer diameter (mm).
pub fn washer_outer_dia(size: &str) -> f64 {
    match size {
        "M2" => 5.0,
        "M2.5" => 6.0,
        "M3" => 7.0,
        "M4" => 9.0,
        "M5" => 10.0,
        "M6" => 12.0,
        "M8" => 16.0,
        "M10" => 20.0,
        "M12" => 24.0,
        "M16" => 30.0,
        "M20" => 37.0,
        _ => 12.0,
    }
}

/// ISO metric size → ISO 7089 flat washer thickness (mm).
pub fn washer_thickness(size: &str) -> f64 {
    match size {
        "M2" | "M2.5" | "M3" => 0.5,
        "M4" | "M5" => 1.0,
        "M6" | "M8" => 1.6,
        "M10" | "M12" => 2.0,
        "M16" | "M20" => 3.0,
        _ => 1.6,
    }
}

/// ISO metric size → hex nut across-flats width (mm).
pub fn nut_across_flats(size: &str) -> f64 {
    match size {
        "M2" => 4.0,
        "M2.5" => 5.0,
        "M3" => 5.5,
        "M4" => 7.0,
        "M5" => 8.0,
        "M6" => 10.0,
        "M8" => 13.0,
        "M10" => 16.0,
        "M12" => 18.0,
        "M16" => 24.0,
        "M20" => 30.0,
        _ => 10.0,
    }
}

/// ISO metric size → hex nut thickness (mm).
pub fn nut_thickness(size: &str) -> f64 {
    match size {
        "M2" => 1.6,
        "M2.5" => 2.0,
        "M3" => 2.4,
        "M4" => 3.2,
        "M5" => 4.7,
        "M6" => 5.2,
        "M8" => 6.8,
        "M10" => 8.4,
        "M12" => 10.8,
        "M16" => 14.8,
        "M20" => 18.0,
        _ => 5.2,
    }
}

/// ISO metric size → socket-head cap screw head diameter (mm).
pub fn socket_head_dia(size: &str) -> f64 {
    match size {
        "M2" => 3.8,
        "M2.5" => 4.5,
        "M3" => 5.5,
        "M4" => 7.0,
        "M5" => 8.5,
        "M6" => 10.0,
        "M8" => 13.0,
        "M10" => 16.0,
        "M12" => 18.0,
        "M16" => 24.0,
        "M20" => 30.0,
        _ => 10.0,
    }
}

/// ISO metric size → socket-head cap screw head height (mm).
pub fn socket_head_height(size: &str) -> f64 {
    match size {
        "M2" => 2.0,
        "M2.5" => 2.5,
        "M3" => 3.0,
        "M4" => 4.0,
        "M5" => 5.0,
        "M6" => 6.0,
        "M8" => 8.0,
        "M10" => 10.0,
        "M12" => 12.0,
        "M16" => 16.0,
        "M20" => 20.0,
        _ => 6.0,
    }
}

/// ISO metric size → hex socket across-flats (mm).
pub fn socket_hex_width(size: &str) -> f64 {
    match size {
        "M2" => 1.5,
        "M2.5" => 2.0,
        "M3" => 2.5,
        "M4" => 3.0,
        "M5" => 4.0,
        "M6" => 5.0,
        "M8" => 6.0,
        "M10" => 8.0,
        "M12" => 10.0,
        "M16" => 14.0,
        "M20" => 17.0,
        _ => 5.0,
    }
}
