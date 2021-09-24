// cxx-async2/include/libunifex_example.h

#ifndef CXX_ASYNC2_LIBUNIFEX_EXAMPLE_H
#define CXX_ASYNC2_LIBUNIFEX_EXAMPLE_H

#include "rust/cxx.h"

struct RustFutureF64;
struct RustFutureString;

rust::Box<RustFutureF64> libunifex_dot_product();
void libunifex_call_rust_dot_product();
void libunifex_schedule_rust_dot_product();
rust::Box<RustFutureF64> libunifex_not_product();
void libunifex_call_rust_not_product();
rust::Box<RustFutureString> libunifex_ping_pong(int i);

#endif
