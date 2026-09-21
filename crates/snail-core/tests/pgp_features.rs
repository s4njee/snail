#[test]
fn rpgp_is_exactly_pinned_without_default_or_asm_features() {
    let manifest = std::fs::read_to_string(format!("{}/Cargo.toml", env!("CARGO_MANIFEST_DIR")))
        .expect("snail-core manifest");
    assert!(manifest.contains("pgp = { version = \"=0.20.0\", default-features = false }"));

    let output = std::process::Command::new(env!("CARGO"))
        .args(["tree", "-e", "features", "-p", "snail-core"])
        .current_dir(format!("{}/../..", env!("CARGO_MANIFEST_DIR")))
        .output()
        .expect("cargo tree");
    assert!(output.status.success());
    let tree = String::from_utf8_lossy(&output.stdout);
    assert!(
        !tree
            .lines()
            .any(|line| line.contains("pgp feature \"asm\"")),
        "{tree}"
    );
    assert!(
        !tree
            .lines()
            .any(|line| line.contains("pgp feature \"default\"")),
        "{tree}"
    );
    let output = std::process::Command::new(env!("CARGO"))
        .args(["tree", "-e", "features", "-p", "pgp@0.20.0"])
        .current_dir(format!("{}/../..", env!("CARGO_MANIFEST_DIR")))
        .output()
        .expect("cargo tree for pgp");
    assert!(output.status.success());
    let pgp_tree = String::from_utf8_lossy(&output.stdout);
    assert!(
        !pgp_tree
            .lines()
            .any(|line| line.contains("cc feature") || line.contains("cc v")),
        "the rpgp graph unexpectedly pulled a C compiler edge:\n{pgp_tree}"
    );
}
