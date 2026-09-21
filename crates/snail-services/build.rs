//! Bake the Google OAuth client into the binary at build time (plan.md E3.1: injected, never
//! committed). The source, in order: `SNAIL_GOOGLE_CLIENT_ID` / `SNAIL_GOOGLE_CLIENT_SECRET` in the
//! build environment, then the gitignored `.secrets/google-oauth.json` at the workspace root —
//! the JSON Google Cloud Console downloads for a Desktop client. With neither, nothing is baked
//! in and onboarding asks for a client instead.

use std::path::Path;

fn main() {
    println!("cargo:rerun-if-env-changed=SNAIL_GOOGLE_CLIENT_ID");
    println!("cargo:rerun-if-env-changed=SNAIL_GOOGLE_CLIENT_SECRET");
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.secrets/google-oauth.json");
    println!("cargo:rerun-if-changed={}", file.display());

    if std::env::var_os("SNAIL_GOOGLE_CLIENT_ID").is_some() {
        // Already in the environment, so `option_env!` sees it without help.
        return;
    }
    let Ok(text) = std::fs::read_to_string(&file) else {
        return;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        println!(
            "cargo:warning={} is not JSON; no Google client baked in",
            file.display()
        );
        return;
    };
    let section = value.get("installed").unwrap_or(&value);
    let field = |name: &str| section.get(name).and_then(|v| v.as_str()).map(str::trim);
    match (field("client_id"), field("client_secret")) {
        (Some(id), Some(secret)) if !id.is_empty() && !secret.is_empty() => {
            println!("cargo:rustc-env=SNAIL_GOOGLE_CLIENT_ID={id}");
            println!("cargo:rustc-env=SNAIL_GOOGLE_CLIENT_SECRET={secret}");
        }
        _ => println!(
            "cargo:warning={} has no client_id/client_secret; no Google client baked in",
            file.display()
        ),
    }
}
