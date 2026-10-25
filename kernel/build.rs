use std::path::PathBuf;

fn main() {
    let here = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    println!(
        "cargo:rustc-link-arg-bins=-T{}",
        here.join("linker.ld").display()
    );
    println!("cargo:rustc-link-arg-bins=--no-pie");
    println!("cargo:rerun-if-changed=linker.ld");

    // The user programs are baked into the kernel image (they become /bin/*).
    // `make` builds them first; we just tell cargo to rebuild when they change.
    let user = here.join("../target/x86_64-unknown-none/release");
    for app in ["init", "gsh", "primes", "life", "counter", "flaky"] {
        let p = user.join(app);
        if !p.exists() {
            panic!(
                "user program {app} missing at {} - run `make` instead of cargo directly",
                p.display()
            );
        }
        println!("cargo:rerun-if-changed={}", p.display());
    }
}
