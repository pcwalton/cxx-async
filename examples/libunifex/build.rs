// cxx-async2/examples/libunifex/build.rs

use pkg_config::Config;

fn main() {
    let libunifex = Config::new().probe("libunifex").unwrap();

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=include/libunifex_example.h");
    println!("cargo:rerun-if-changed=src/libunifex_example.cpp");

    cxx_build::bridge("src/main.rs")
        .file("src/libunifex_example.cpp")
        .include("include")
        .include("../common/include")
        .include("../../cxx-async/include")
        .includes(&libunifex.include_paths)
        .compile("libunifex_example");
}
