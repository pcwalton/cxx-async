// cxx-async/build.rs

use std::fs;
use std::path::PathBuf;

fn main() {
    let sources = vec!["src/cxx_async.cpp"];
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

    let no_bridges: Vec<PathBuf> = vec![];
    cxx_build::bridges(no_bridges)
        .files(&sources)
        .include("include")
        .compile("cxx-async");
}
