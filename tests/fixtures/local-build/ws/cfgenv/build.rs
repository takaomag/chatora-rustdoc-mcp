fn main() {
    println!("cargo::rustc-check-cfg=cfg(from_build)");
    println!("cargo::rustc-cfg=from_build");
    println!("cargo::rustc-env=FROM_BUILD=yes");
}
