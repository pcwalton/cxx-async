// cxx-async/include/cppcoro_example.h

#ifndef CXX_ASYNC_CPPCORO_EXAMPLE_H
#define CXX_ASYNC_CPPCORO_EXAMPLE_H

#include "rust/cxx.h"

struct RustFutureF64;
struct RustFutureString;

rust::Box<RustFutureF64> cppcoro_dot_product();
void cppcoro_call_rust_dot_product();
void cppcoro_schedule_rust_dot_product();
rust::Box<RustFutureF64> cppcoro_not_product();
void cppcoro_call_rust_not_product();
rust::Box<RustFutureString> cppcoro_ping_pong(int i);

#endif  // CXX_ASYNC_CPPCORO_EXAMPLE_H
