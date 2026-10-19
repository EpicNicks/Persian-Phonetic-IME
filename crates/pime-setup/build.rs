// Windows "installer detection" auto-elevates manifest-less 32-bit executables whose name contains
// "setup" (pime-setup.x86.exe, and even the x86 unit-test binary pime_setup-*.exe). Embedding an
// asInvoker manifest switches that heuristic off, so `/status` and `/enable-user` run as the user.
fn main() {
    let target = std::env::var("TARGET").unwrap_or_default();
    if target.ends_with("-windows-msvc") {
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg=/MANIFESTUAC:level='asInvoker'");
    }
    println!("cargo:rerun-if-changed=build.rs");
}
