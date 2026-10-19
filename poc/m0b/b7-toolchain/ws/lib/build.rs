//! Sets `build_saw_foo` when the build script was compiled with `--cfg foo`.

fn main() {
    println!("cargo:rustc-check-cfg=cfg(build_saw_foo)");
    println!("cargo:rustc-check-cfg=cfg(foo)");
    if cfg!(foo) {
        println!("cargo:rustc-cfg=build_saw_foo");
    }
}
