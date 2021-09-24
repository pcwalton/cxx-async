// cxx-async2/build.rs

use pkg_config::Config;
use std::fs;

fn main() {
    /*
    let _ = Config::new()
        .atleast_version("0.58.0")
        .probe("libfolly");
        */
    let cppcoro = Config::new().probe("cppcoro").unwrap();
    println!("cargo:rustc-link-lib=cxxbridge1");

    // TODO(pcwalton): Conditionally enable Folly example.
    let sources = vec!["src/cppcoro_example.cpp", "src/cxx_async.cpp"];
    for source in &sources {
        println!("cargo:rerun-if-changed={}", source);
    }
    for dirent in fs::read_dir("include").unwrap() {
        if let Ok(dirent) = dirent {
            if dirent.file_type().map(|ft| ft.is_file()).unwrap_or(false) {
                println!("cargo:rerun-if-changed={:?}", dirent.path());
            }
        }
    }

    cxx_build::bridge("src/main.rs")
        .files(&sources)
        .include("include")
        .includes(cppcoro.include_paths)
        .compile("cxx-async");
}
