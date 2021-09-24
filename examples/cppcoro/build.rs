// cxx-async2/examples/cppcoro/build.rs

use pkg_config::Config;
use std::fs;

fn main() {
    /*
    let _ = Config::new()
        .atleast_version("0.58.0")
        .probe("libfolly");
        */
    let cppcoro = Config::new().probe("cppcoro").unwrap();

    println!("cargo:rerun-if-changed=src/cppcoro_example.cpp");
    /*
    for dirent in fs::read_dir("include").unwrap() {
        if let Ok(dirent) = dirent {
            if dirent.file_type().map(|ft| ft.is_file()).unwrap_or(false) {
                println!("cargo:rerun-if-changed={:?}", dirent.path());
            }
        }
    }
    */

    cxx_build::bridge("src/main.rs")
        .file("src/cppcoro_example.cpp")
        .include("include")
        .include("../common/include")
        .include("../../cxx-async/include")
        .includes(&cppcoro.include_paths)
        .compile("cxx-async");
}
