#[test]
fn calcard_is_exactly_pinned_without_its_jmap_default() {
    let manifest = include_str!("../Cargo.toml");
    assert!(manifest.contains("calcard = { version = \"=0.3.14\", default-features = false }"));
}
