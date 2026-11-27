use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "none" {
        return;
    }

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir.join("..");

    if env::var_os("CARGO_FEATURE_BOARD_RASPI3B").is_some() {
        let linker_script = workspace_root
            .join("hal")
            .join("board-raspi3b")
            .join("linker.ld");
        println!("cargo:rerun-if-changed={}", linker_script.display());
        println!("cargo:rustc-link-arg=-T{}", linker_script.display());
    }

    // Build EL0 userspace ELF binaries (`drv-uart` and `init`) into a separate target dir
    // so Cargo never deadlocks on the outer workspace build lock.
    for watched in [
        "drivers/drv-uart/src/main.rs",
        "drivers/drv-uart/Cargo.toml",
        "userland/init/src/main.rs",
        "userland/init/Cargo.toml",
        "userland/libdelta/src/lib.rs",
        "userland/delta-abi/src/lib.rs",
        "userland/user.ld",
    ] {
        println!(
            "cargo:rerun-if-changed={}",
            workspace_root.join(watched).display()
        );
    }

    let userland_target = workspace_root.join("target").join("userland");
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let status = Command::new(cargo)
        .current_dir(&workspace_root)
        .args([
            "build",
            "--package",
            "drv-uart",
            "--package",
            "init",
            "--target",
            "aarch64-unknown-none",
            "--release",
            "--target-dir",
        ])
        .arg(&userland_target)
        .status()
        .expect("failed to invoke cargo for EL0 userland binaries");

    assert!(status.success(), "building EL0 userland binaries failed");

    let elf_dir = userland_target.join("aarch64-unknown-none").join("release");
    println!(
        "cargo:rustc-env=DRV_UART_ELF={}",
        elf_dir.join("drv-uart").display()
    );
    println!(
        "cargo:rustc-env=INIT_ELF={}",
        elf_dir.join("init").display()
    );
}
