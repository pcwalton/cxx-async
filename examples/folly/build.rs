// cxx-async/examples/folly/build.rs

fn main() {
    let folly = find_folly::probe_folly().expect("Couldn't find the Folly library!");

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=include/folly_example.h");
    println!("cargo:rerun-if-changed=src/folly_example.cpp");
    println!("cargo:rustc-link-lib=atomic");

    let mut build = cxx_build::bridge("src/main.rs");
    build
        .file("src/folly_example.cpp")
        .include("include")
        .include("../common/include")
        .include("../../cxx-async/include")
        .includes(&folly.include_paths);
    build.flag("-std=c++20");
    build.flag("-Wno-ignored-qualifiers");

    for other_cflag in &folly.other_cflags {
        build.flag(other_cflag);
    }

    build.compile("folly_example");
}
