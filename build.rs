use std::env;
use std::path::Path;
use std::process::Command;

fn main() {
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }

    if let Some(swift_lib_path) = swift_lib_path() {
        println!("cargo:rustc-link-arg=-Wl,-rpath,{swift_lib_path}");
    } else {
        println!("cargo:warning=Could not locate Swift toolchain libraries for rpath");
    }

    // System Swift libraries on newer macOS versions (Swift Concurrency runtime).
    println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
}

fn swift_lib_path() -> Option<String> {
    let output = Command::new("xcrun")
        .args(["--toolchain", "default", "--find", "swift"])
        .output()
        .ok()?;

    let swift_path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if swift_path.is_empty() {
        return None;
    }

    let toolchain_path = Path::new(&swift_path).parent()?.parent()?;
    let lib_path = toolchain_path.join("lib/swift/macosx");
    if lib_path.exists() {
        Some(lib_path.to_string_lossy().into_owned())
    } else {
        None
    }
}
