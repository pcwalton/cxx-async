// cxx-async/build.rs

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    // Copy include files so that dependent crates can find them.

    let out_dir = Path::new(&env::var("OUT_DIR").expect("`OUT_DIR` not set!")).to_owned();
    let mut src_include_path = Path::new(".").to_owned();
    let mut dest_include_path = out_dir.clone();
    for include_path in &mut [&mut src_include_path, &mut dest_include_path] {
        include_path.push("include");
        include_path.push("rust");
    }

    drop(fs::create_dir_all(&dest_include_path));
    for header in &["cxx_async.h", "cxx_async_cppcoro.h", "cxx_async_folly.h"] {
        drop(fs::copy(
            Path::join(&src_include_path, header),
            Path::join(&dest_include_path, header),
        ));
    }
    println!(
        "cargo:include={}",
        Path::join(&out_dir, "include").to_string_lossy()
    );

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/cxx_async.cpp");
    println!("cargo:rerun-if-changed=include/rust/cxx_async.h");
    println!("cargo:rerun-if-changed=include/rust/cxx_async_cppcoro.h");
    println!("cargo:rerun-if-changed=include/rust/cxx_async_folly.h");

    println!("cargo:rustc-cfg=built_with_cargo");

    let no_bridges: Vec<PathBuf> = vec![];
    cxx_build::bridges(no_bridges)
        .warnings(false)
        .cargo_warnings(false)
        .files(&vec!["src/cxx_async.cpp"])
        .flag_if_supported("-std=c++20")
        .include("include")
        .compile("cxx-async");
}
