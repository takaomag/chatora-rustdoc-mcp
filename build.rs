//! Embeds the target this server is built for. It stands in for the host target when `rustc -vV`
//! cannot tell it (requirement R-TG-1, decided in M1).

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    // Cargo always sets TARGET for a build script; an empty value makes the fallback unusable.
    let target = std::env::var("TARGET").unwrap_or_default();
    // Not under `CHATORA_RUSTDOC_MCP_`, the prefix of the settings (requirement R-DIST-3): cargo
    // sets it for `cargo run` and `cargo test` too, where the server would take it for one.
    println!("cargo::rustc-env=CHATORA_RUSTDOC_BUILD_TARGET={target}");
}
