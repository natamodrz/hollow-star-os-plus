use std::env;
use std::path::PathBuf;

fn main() {
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "none" {
        return;
    }

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));

    if env::var_os("CARGO_FEATURE_BOARD_RASPI3B").is_some() {
        let linker_script = manifest_dir
            .join("..")
            .join("hal")
            .join("board-raspi3b")
            .join("linker.ld");
        println!("cargo:rerun-if-changed={}", linker_script.display());
        println!("cargo:rustc-link-arg=-T{}", linker_script.display());
    }
}
