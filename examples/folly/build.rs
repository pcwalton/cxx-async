// cxx-async/examples/folly/build.rs

use pkg_config::Config;
use shlex::Shlex;
use std::path::PathBuf;
use std::process::Command;
use std::str::FromStr;

fn main() {
    // Folly's `.pc` file is missing the `fmt` and `gflags` dependencies. Find them here.
    Config::new()
        .statik(true)
        .probe("fmt")
        .expect("No `fmt` package found!");
    Config::new().statik(true).probe("gflags").expect("No `gflags` package found!");

    // Unfortunately, the `pkg-config` crate doesn't successfully parse some of Folly's
    // dependencies, because it passes the raw `.so` files instead of using `-l` flags. So call
    // `pkg-config` manually.
    let mut lib_dirs = vec![];
    let output = Command::new("pkg-config")
        .args(&["--static", "--libs", "libfolly"])
        .output()
        .expect("Failed to execute `pkg-config` to find Folly!");
    let output = String::from_utf8(output.stdout).expect("`pkg-config --libs` wasn't UTF-8!");
    for arg in Shlex::new(&output) {
        if arg.starts_with("-") {
            if arg.starts_with("-L") {
                lib_dirs.push(PathBuf::from(&arg[2..]));
            } else if arg.starts_with("-l") {
                println!("cargo:rustc-link-lib={}", &arg[2..]);
            }
            continue;
        }

        let path = PathBuf::from_str(&arg).unwrap();
        let (parent, lib_name) = match (path.parent(), path.file_stem()) {
            (Some(parent), Some(lib_name)) => (parent, lib_name),
            _ => continue,
        };
        let lib_name = lib_name.to_string_lossy();
        if lib_name.starts_with("lib") {
            println!("cargo:rustc-link-search={}", parent.display());
            println!("cargo:rustc-link-lib={}", &lib_name[3..]);
        }
    }

    // Unfortunately, just like `fmt` and `gflags`, Folly's `.pc` file doesn't contain a link flag
    // for `boost_context`. What's worse, the name varies based on different systems
    // (`libboost_context.a` vs.  `libboost_context-mt.a`). So find that library manually. We assume
    // it's in the same directory as the Folly installation itself.
    let mut found_boost_context = false;
    for lib_dir in &lib_dirs {
        println!("cargo:rustc-link-search={}", lib_dir.display());

        if found_boost_context {
            continue;
        }
        for possible_lib_name in &["boost_context", "boost_context-mt"] {
            let mut lib_dir = (*lib_dir).clone();
            lib_dir.push(&format!("lib{}.a", possible_lib_name));
            if !lib_dir.exists() {
                continue;
            }
            println!("cargo:rustc-link-lib={}", possible_lib_name);
            found_boost_context = true;
            break;
        }
    }
    if !found_boost_context {
        panic!("Could not find `boost_context`. Make sure either `libboost_context.a` or \
            `libboost_context-mt.a` is located in the same directory as Folly.")
    }

    let output = Command::new("pkg-config")
        .args(&["--static", "--cflags", "libfolly"])
        .output()
        .expect("Failed to execute `pkg-config` to find Folly!");
    let output = String::from_utf8(output.stdout).expect("`pkg-config --libs` wasn't UTF-8!");
    let mut include_dirs = vec![];
    for arg in output.split_whitespace() {
        if arg.starts_with("-I") {
            include_dirs.push(arg[2..].to_owned());
        }
    }

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=include/folly_example.h");
    println!("cargo:rerun-if-changed=src/folly_example.cpp");

    cxx_build::bridge("src/main.rs")
        .file("src/folly_example.cpp")
        .include("include")
        .include("../common/include")
        .include("../../cxx-async/include")
        .includes(&include_dirs)
        .compile("folly_example");
}
