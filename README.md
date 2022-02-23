# `cxx-async`

## Overview

`cxx-async` is a Rust crate that extends the [`cxx`](http://cxx.rs/) library to provide
interoperability between asynchronous Rust code using `async`/`await` and [C++20 coroutines] using
`co_await`. If your C++ code is asynchronous, `cxx-async` can provide a more convenient, and
potentially more efficient, alternative to callbacks. You can freely convert between C++ coroutines
and Rust futures and/or streams and await one from the other.

It's important to emphasize what `cxx-async` isn't: it isn't a C++ binding to Tokio or any other
Rust I/O library. Nor is it a Rust binding to `boost::asio` or similar. Such bindings could in
principle be layered on top of `cxx-async` if desired, but this crate doesn't provide them out of
the box. (Note that this is a tricky problem even in theory, since Rust async I/O code is generally
tightly coupled to a single library such as Tokio, in much the same way C++ async I/O code tends to
be tightly coupled to libraries like `boost::asio`.) If you're writing server code, you can still
use `cxx-async`, but you will need to ensure that both the Rust and C++ sides run separate I/O
executors.

`cxx-async` aims for compatibility with popular C++ coroutine support libraries. Right now, both
the lightweight [`cppcoro`](https://github.com/lewissbaker/cppcoro) and the more comprehensive
[Folly](https://github.com/facebook/folly/) are supported. Pull requests are welcome to support
others.

## Quick tutorial

To use `cxx-async`, first start by adding `cxx` to your project. Then add the following to your
`Cargo.toml`:

```toml
[dependencies]
cxx-async = "0.1"
```

Now, inside your `#[cxx::bridge]` module, declare a future type and some methods like so:

```rust
#[cxx::bridge]
mod ffi {
    // Declare type aliases for each of the future types you wish to use here. Then declare
    // async C++ methods that you wish Rust to call. Make sure they return one of the future
    // types you declared.
    unsafe extern "C++" {
        type RustFutureString = crate::RustFutureString;

        fn hello_from_cpp() -> RustFutureString;
    }

    // Async Rust methods that you wish C++ to call go here. Again, make sure they return one of the
    // boxed future types you declared above.
    extern "Rust" {
        fn hello_from_rust() -> Box<RustFutureString>;
    }
}
```

After the `#[cxx::bridge]` block, define the future types using the `#[cxx_async::bridge]`
attribute:

```rust
// The inner type is the Rust type that this future yields.
#[cxx_async::bridge]
unsafe impl Future for RustFutureString {
    type Output = String;
}
```

Now, in your C++ file, make sure to `#include` the right headers:

```cpp
#include "rust/cxx.h"
#include "rust/cxx_async.h"
#include "rust/cxx_async_cppcoro.h"  // Or cxx_async_folly.h, as appropriate.
```

And add a call to the `CXXASYNC_DEFINE_FUTURE` macro to define the C++ side of the future:

```cpp
// The first argument is the C++ type that the future yields, and the second argument is the
// fully-qualified name of the future, with `::` namespace separators replaced with commas. (For
// instance, if your future is named `mycompany::myproject::RustFutureString`, you might write
// `CXXASYNC_DEFINE_FUTURE(rust::String, mycompany, myproject, RustFutureString);`. The first
// argument is the C++ type that `cxx` maps your Rust type to: in this case, `String` maps to
// `rust::String`, so we supply `rust::String` here.
//
// This macro must be invoked at the top level, not in a namespace.
CXXASYNC_DEFINE_FUTURE(rust::String, RustFutureString);
```

You're all set! Now you can define asynchronous C++ code that Rust can call:

```cpp
RustFutureString hello_from_cpp() {
    co_return std::string("Hello world!");
}
```

On the Rust side:

```rust
async fn call_cpp() -> String {
    // This returns a Result (with the error variant populated if C++ threw an exception), so you
    // need to unwrap it:
    ffi::hello_from_cpp().await.unwrap()
}
```

And likewise, define some asynchronous Rust code that C++ can call:

```rust
use cxx_async::CxxAsyncResult;
fn hello_from_rust() -> RustFutureString {
    // You can instead use `fallible` if your async block returns a Result.
    RustFutureString::infallible(async { "Hello world!".to_owned() })
}
```

Over on the C++ side:

```cpp
cppcoro::task<rust::String> call_rust() {
    co_return hello_from_rust();
}
```

That's it! You should now be able to freely await futures on either side. An analogous procedure can
be followed to wrap C++ coroutines that yield values with `co_yield` in Rust streams.

## Installation notes

You will need a C++ compiler that implements the coroutines TS, which generally coincides with
support for C++20. Some C++ compilers (e.g. Apple clang 13.0.0) that implement the coroutines TS
crash when compiling Folly. It's also recommended to use `libc++` instead of `libstdc++`, as the
former has more complete support for coroutines.

Usage of `cxx-async` with Folly requires that Folly have been built with coroutine support. This
generally means that you need to build Folly with `-DCXX_STD=20`. Many distributions of Folly (e.g.
the one in Homebrew) don't have coroutine support enabled; a common symptom of this is a linker
error mentioning a missing symbol `folly::resumeCoroutineWithNewAsyncStackRoot`.

## Code of conduct

`cxx-async` follows the same Code of Conduct as Rust itself. Reports can be made to the crate
authors.

## License

Licensed under either of Apache License, Version 2.0 or MIT license at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in
this project by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without
any additional terms or conditions.

[C++20 coroutines]: https://en.cppreference.com/w/cpp/language/coroutines
