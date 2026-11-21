use nickel_util::json;
#[test]
fn explicit_json_limits_preserve_default_bounds_and_duplicate_refusal(
) -> Result<(), Box<dyn std::error::Error>> {
    let input = serde_json::to_vec(&vec![0; 10_001])?;
    assert!(json::parse(&input).is_err());
    assert!(json::parse_with_limits(&input, 64 * 1024, 20_000).is_ok());
    assert!(json::parse_with_limits(&input, 100, 20_000).is_err());
    assert!(json::parse_with_limits(&input, 64 * 1024, 10_000).is_err());
    assert!(json::parse_with_limits(br#"{"a":1,"a":2}"#, 64 * 1024, 20_000).is_err());
    let nested = format!("{}0{}", "[".repeat(66), "]".repeat(66));
    assert!(json::parse_with_limits(nested.as_bytes(), 64 * 1024, 20_000).is_err());
    Ok(())
}
