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

    if env::var_os("CARGO_FEATURE_BOARD_RASPI3B").is_some()
        || env::var_os("CARGO_FEATURE_BOARD_RASPI02W").is_some()
    {
        let linker_script = workspace_root
            .join("hal")
            .join("board-raspi3b")
            .join("linker.ld");
        println!("cargo:rerun-if-changed={}", linker_script.display());
        println!("cargo:rustc-link-arg=-T{}", linker_script.display());
    }

    // Build EL0 userspace binaries into a separate target dir so Cargo never deadlocks.
    for watched in [
        "drivers/drv-uart/src/main.rs",
        "drivers/drv-uart/Cargo.toml",
        "drivers/drv-sd/src/main.rs",
        "drivers/drv-sd/Cargo.toml",
        "drivers/drv-fb/src/main.rs",
        "drivers/drv-fb/Cargo.toml",
        "drivers/drv-audio/src/main.rs",
        "drivers/drv-audio/Cargo.toml",
        "drivers/drv-kbd/src/main.rs",
        "drivers/drv-kbd/Cargo.toml",
        "fs/src/lib.rs",
        "fs/src/main.rs",
        "fs/src/deltafs.rs",
        "fs/src/fat32.rs",
        "fs/src/ext2.rs",
        "fs/src/mbr.rs",
        "fs/Cargo.toml",
        "userland/init/src/main.rs",
        "userland/init/Cargo.toml",
        "userland/shell/src/main.rs",
        "userland/shell/Cargo.toml",
        "apps/hello/src/main.rs",
        "apps/hello/Cargo.toml",
        "apps/compositor/src/main.rs",
        "apps/compositor/Cargo.toml",
        "apps/setup/src/main.rs",
        "apps/setup/Cargo.toml",
        "apps/launcher/src/main.rs",
        "apps/launcher/Cargo.toml",
        "apps/terminal/src/main.rs",
        "apps/terminal/Cargo.toml",
        "apps/hollow-star/src/main.rs",
        "apps/hollow-star/src/calc.rs",
        "apps/hollow-star/src/games.rs",
        "apps/hollow-star/Cargo.toml",
        "userland/libdelta-ui/src/lib.rs",
        "userland/libdelta-ui/src/calibri.rs",
        "userland/libdelta-ui/src/fx.rs",
        "userland/libdelta-ui/src/gfx.rs",
        "userland/libdelta-ui/src/icons.rs",
        "userland/libdelta-ui/src/i18n.rs",
        "userland/libdelta-ui/src/client.rs",
        "userland/libdelta-ui/Cargo.toml",
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
            "drv-sd",
            "--package",
            "drv-fb",
            "--package",
            "drv-audio",
            "--package",
            "drv-kbd",
            "--package",
            "vfs-server",
            "--package",
            "init",
            "--package",
            "shell",
            "--package",
            "hello-app",
            "--package",
            "compositor",
            "--package",
            "setup-app",
            "--package",
            "launcher-app",
            "--package",
            "terminal-app",
            "--package",
            "hollow-star-app",
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
        "cargo:rustc-env=DRV_SD_ELF={}",
        elf_dir.join("drv-sd").display()
    );
    println!(
        "cargo:rustc-env=DRV_FB_ELF={}",
        elf_dir.join("drv-fb").display()
    );
    println!(
        "cargo:rustc-env=DRV_AUDIO_ELF={}",
        elf_dir.join("drv-audio").display()
    );
    println!(
        "cargo:rustc-env=DRV_KBD_ELF={}",
        elf_dir.join("drv-kbd").display()
    );
    println!(
        "cargo:rustc-env=VFSD_ELF={}",
        elf_dir.join("vfsd").display()
    );
    println!(
        "cargo:rustc-env=INIT_ELF={}",
        elf_dir.join("init").display()
    );
}
