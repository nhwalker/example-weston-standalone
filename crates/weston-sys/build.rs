use std::fs;
use std::path::Path;

fn main() {
    // Resolve the installed RPM headers/libs.  This links libweston-14 and
    // wayland-server for every downstream crate (plan §2).
    let libweston = pkg_config::Config::new()
        .atleast_version("14.0.1")
        .probe("libweston-14")
        .expect(
            "libweston-14.pc not found — build inside the westonite build container (weston-devel)",
        );
    let wayland = pkg_config::Config::new()
        .probe("wayland-server")
        .expect("wayland-server.pc not found");

    // Version tripwire (plan §6, risk R-C): the committed bindings.rs
    // records the libweston version it was generated from; a pkg-config
    // mismatch means EPEL moved and the regen script must be re-run.
    let bindings = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/bindings.rs");
    let text = fs::read_to_string(&bindings)
        .expect("src/bindings.rs missing — run scripts/regen-bindings.sh in the build container");
    let recorded = text
        .lines()
        .find_map(|l| l.strip_prefix("// libweston-modversion: "))
        .expect("bindings.rs lacks the libweston-modversion marker — regenerate it")
        .trim();
    assert_eq!(
        recorded, libweston.version,
        "libweston version skew: bindings.rs was generated against {recorded} but \
         pkg-config reports {}. Run scripts/regen-bindings.sh and re-verify the \
         §3 header facts (docs/rust-migration-plan.md §8).",
        libweston.version
    );

    // The C shim (§3k) + optional fake-C-object test harness (D18).
    let mut cc = cc::Build::new();
    cc.file("shim/shim.c");
    if std::env::var_os("CARGO_FEATURE_TESTSUPPORT").is_some() {
        cc.file("shim/testsupport.c");
    }
    for inc in libweston.include_paths.iter().chain(&wayland.include_paths) {
        cc.include(inc);
    }
    cc.warnings(true).compile("weston-sys-shim");

    println!("cargo:rerun-if-changed=shim/shim.c");
    println!("cargo:rerun-if-changed=shim/shim.h");
    println!("cargo:rerun-if-changed=shim/testsupport.c");
    println!("cargo:rerun-if-changed=shim/testsupport.h");
    println!("cargo:rerun-if-changed=src/bindings.rs");
}
