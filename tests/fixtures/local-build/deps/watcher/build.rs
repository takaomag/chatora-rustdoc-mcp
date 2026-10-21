// As proc-macro2's: a build script that is run again when the value changes.
fn main() {
    println!("cargo::rerun-if-env-changed=RUSTC_BOOTSTRAP");
}
