// cxx-async/include/folly_example.h

#ifndef CXX_ASYNC_FOLLY_EXAMPLE_H
#define CXX_ASYNC_FOLLY_EXAMPLE_H

#include "rust/cxx.h"

struct RustFutureF64;
struct RustFutureString;

rust::Box<RustFutureF64> folly_dot_product();
void folly_call_rust_dot_product();
void folly_schedule_rust_dot_product();
rust::Box<RustFutureF64> folly_not_product();
void folly_call_rust_not_product();
rust::Box<RustFutureString> folly_ping_pong(int i);

#endif  // CXX_ASYNC_FOLLY_EXAMPLE_H
