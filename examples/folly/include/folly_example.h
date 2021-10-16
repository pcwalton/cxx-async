// cxx-async/include/folly_example.h

#ifndef CXX_ASYNC_FOLLY_EXAMPLE_H
#define CXX_ASYNC_FOLLY_EXAMPLE_H

#include "rust/cxx.h"

struct RustFutureF64;
struct RustFutureString;

namespace foo {
namespace bar {
struct RustFutureStringNamespaced;
}
}

rust::Box<RustFutureF64> folly_dot_product_coro();
rust::Box<RustFutureF64> folly_dot_product_futures();
rust::Box<foo::bar::RustFutureStringNamespaced> folly_get_namespaced_string();
double folly_call_rust_dot_product();
double folly_schedule_rust_dot_product();
rust::Box<RustFutureF64> folly_not_product();
rust::String folly_call_rust_not_product();
rust::Box<RustFutureString> folly_ping_pong(int i);
void folly_send_to_dropped_future_go();
rust::Box<RustFutureF64> folly_send_to_dropped_future();

#endif  // CXX_ASYNC_FOLLY_EXAMPLE_H
