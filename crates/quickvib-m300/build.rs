//! Build script for `quickvib-m300`.
//!
//! Does nothing at all in a normal build. With the non-default `bindgen` feature it regenerates
//! the FFI declaration snapshot from the vendor header and fails the build if the result differs
//! from what is committed, which is how SDK drift gets caught (`docs/M300-NATIVE.md` 4).

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=QUICKVIB_M300_HEADER");
    println!("cargo:rerun-if-env-changed=QUICKVIB_M300_SDK");

    #[cfg(feature = "bindgen")]
    bindings::generate();
}

/// Developer-bench only: `cargo build -p quickvib-m300 --features bindgen` on a machine that has
/// the SDK header and `libclang`. Never runs in CI.
#[cfg(feature = "bindgen")]
mod bindings {
    use std::path::{Path, PathBuf};

    /// Reviewed snapshot of the generated declarations, committed so the normal build needs
    /// neither `libclang` nor the vendor header. Absent until Phase 6 (blocked on Q-A).
    const SNAPSHOT: &str = "src/ffi_generated.rs";

    /// Header file name expected inside a `QUICKVIB_M300_SDK` directory. **TBD** — confirm against
    /// the SDK v1.2.0 distribution.
    const HEADER_NAME: &str = "m300.h";

    pub(crate) fn generate() {
        let header = header_path();
        println!("cargo:rerun-if-changed={}", header.display());

        let builder = bindgen::Builder::default()
            .header(header.to_string_lossy().into_owned())
            // Bind the vendor's surface and nothing else: every extra declaration is another
            // chance to get the ABI wrong (docs/M300-NATIVE.md 3).
            .allowlist_function("(?i)^m300_.*")
            .allowlist_type("(?i)^m300_.*")
            .allowlist_var("(?i)^m300_.*")
            .generate_comments(true)
            .layout_tests(false)
            .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()));

        let generated = match builder.generate() {
            Ok(bindings) => bindings.to_string(),
            Err(err) => panic!("bindgen failed on {}: {err}", header.display()),
        };

        let out_dir = match std::env::var_os("OUT_DIR") {
            Some(dir) => PathBuf::from(dir),
            None => panic!("OUT_DIR is not set; this is not a cargo build"),
        };
        let fresh = out_dir.join("ffi_generated.rs");
        if let Err(err) = std::fs::write(&fresh, &generated) {
            panic!("could not write {}: {err}", fresh.display());
        }

        compare_with_snapshot(&generated, &fresh);
    }

    /// Fail loudly on drift rather than silently rewriting reviewed source: the snapshot is the
    /// declaration set a human checked against the header, so a change to it is a review event.
    fn compare_with_snapshot(generated: &str, fresh: &Path) {
        let snapshot = Path::new(SNAPSHOT);
        match std::fs::read_to_string(snapshot) {
            Ok(committed) if committed == generated => {
                println!("cargo:warning=bindgen: {SNAPSHOT} is up to date with the SDK header");
            }
            Ok(_) => panic!(
                "bindgen: {SNAPSHOT} differs from the declarations generated from the SDK header. \
                 Review the difference, then copy {} over it and commit the change.",
                fresh.display()
            ),
            Err(_) => println!(
                "cargo:warning=bindgen: {SNAPSHOT} does not exist yet. Review {} and commit it as \
                 the reviewed snapshot (docs/M300-NATIVE.md 4).",
                fresh.display()
            ),
        }
    }

    fn header_path() -> PathBuf {
        if let Some(path) = std::env::var_os("QUICKVIB_M300_HEADER") {
            return PathBuf::from(path);
        }
        if let Some(dir) = std::env::var_os("QUICKVIB_M300_SDK") {
            return Path::new(&dir).join(HEADER_NAME);
        }
        panic!(
            "the `bindgen` feature needs the SDK header: set QUICKVIB_M300_HEADER to the full \
             path of {HEADER_NAME}, or QUICKVIB_M300_SDK to the directory containing it"
        );
    }
}
