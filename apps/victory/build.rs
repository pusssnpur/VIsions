//! Link settings for the Windows executable.

fn main() {
    // Threads created without a stack size take the executable's default.
    // Host graphics drivers compiling the title's large shaders on such
    // threads (vkd3d-proton's and the drivers' own, under Wine too) overflow
    // the linker's default of 1 MiB: reserve 16 MiB instead.
    match std::env::var("CARGO_CFG_TARGET_ENV").as_deref() {
        Ok("msvc") => println!("cargo:rustc-link-arg-bins=/STACK:16777216"),
        Ok("gnu") if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") => {
            println!("cargo:rustc-link-arg-bins=-Wl,--stack,16777216");
        }
        _ => {}
    }
}

