use super::maximized_geometry_needs_repair;

#[test]
fn repairs_large_maximized_geometry_mismatch() {
    assert!(maximized_geometry_needs_repair(604, 1384, 1080, 1501));
    assert!(maximized_geometry_needs_repair(1920, 1000, 3840, 2160));
}

#[test]
fn accepts_taskbar_sized_maximized_work_area() {
    assert!(!maximized_geometry_needs_repair(1920, 1040, 1920, 1080));
    assert!(!maximized_geometry_needs_repair(2560, 1400, 2560, 1440));
}
