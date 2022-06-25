// cxx-async/build.rs

use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/cxx_async.cpp");
    println!("cargo:rerun-if-changed=include/rust/cxx_async.h");
    println!("cargo:rerun-if-changed=include/rust/cxx_async_cppcoro.h");
    println!("cargo:rerun-if-changed=include/rust/cxx_async_folly.h");

    println!("cargo:rustc-cfg=built_with_cargo");

    let no_bridges: Vec<PathBuf> = vec![];
    cxx_build::bridges(no_bridges)
        .files(&vec!["src/cxx_async.cpp"])
        .include("include")
        .compile("cxx-async");
}
