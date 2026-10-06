pub(crate) fn normalize_desktop_axis(
    value: i32,
    origin: i32,
    extent: i32,
) -> Result<i32, &'static str> {
    let local = i64::from(value) - i64::from(origin);
    if extent <= 0 || local < 0 || local >= i64::from(extent) {
        return Err("absolute pointer position outside virtual desktop");
    }
    if extent == 1 {
        return Ok(0);
    }
    let denominator = i64::from(extent) - 1;
    Ok(((local * 65535 + denominator / 2) / denominator) as i32)
}

#[cfg(test)]
mod virtual_desktop_tests {
    use super::*;
    #[test]
    fn negative_origin_and_both_edges() {
        assert_eq!(normalize_desktop_axis(-1920, -1920, 4480).unwrap(), 0);
        assert_eq!(normalize_desktop_axis(2559, -1920, 4480).unwrap(), 65535);
        assert_eq!(normalize_desktop_axis(0, -1920, 4480).unwrap(), 28093);
        assert_eq!(normalize_desktop_axis(-1, -1, 1).unwrap(), 0);
        assert!(normalize_desktop_axis(-1921, -1920, 4480).is_err());
        assert!(normalize_desktop_axis(2560, -1920, 4480).is_err());
        assert!(normalize_desktop_axis(0, 0, 0).is_err());
    }
}
