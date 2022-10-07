/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is licensed under both the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree and the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree.
 */

// cxx-async/examples/folly/include/folly_example.h

#ifndef CXX_ASYNC_FOLLY_EXAMPLE_H
#define CXX_ASYNC_FOLLY_EXAMPLE_H

#include <folly/ExceptionWrapper.h>
#include <exception>
#include <iostream>
#include "rust/cxx.h"
#include "rust/cxx_async.h"

CXXASYNC_DEFINE_FUTURE(void, RustFutureVoid);
CXXASYNC_DEFINE_FUTURE(double, RustFutureF64);
CXXASYNC_DEFINE_FUTURE(rust::String, RustFutureString);
CXXASYNC_DEFINE_FUTURE(rust::String, foo, bar, RustFutureStringNamespaced);
CXXASYNC_DEFINE_STREAM(rust::String, RustStreamString);

class MyException : public std::exception {
  const char* m_message;

 public:
  MyException(const char* message) : m_message(message) {}
  const char* message() const noexcept {
    return m_message;
  }
};

namespace rust {
namespace async {
namespace behavior {

template <typename T>
struct TryCatch<T, Custom> {
  template <typename Try, typename Fail>
  static void trycatch(Try&& func, Fail&& fail) noexcept {
    try {
      func();
    } catch (const MyException& exception) {
      fail(exception.message());
    } catch (const async::Error& exception) {
      fail(exception.what());
    } catch (const std::exception& exception) {
      // Should never get here.
      std::terminate();
    }
  }
};

} // namespace behavior
} // namespace async
} // namespace rust

RustFutureF64 folly_dot_product_coro();
RustFutureF64 folly_dot_product_futures();
void folly_call_rust_hello();
foo::bar::RustFutureStringNamespaced folly_get_namespaced_string();
double folly_call_rust_dot_product();
double folly_schedule_rust_dot_product();
RustFutureF64 folly_not_product();
rust::String folly_call_rust_not_product();
RustFutureString folly_ping_pong(int i);
RustFutureVoid folly_complete();
void folly_send_to_dropped_future_go();
RustFutureF64 folly_send_to_dropped_future();
RustStreamString folly_fizzbuzz();
RustStreamString folly_indirect_fizzbuzz();
RustStreamString folly_not_fizzbuzz();
RustFutureVoid folly_drop_coroutine_wait();
RustFutureVoid folly_drop_coroutine_signal();

#endif // CXX_ASYNC_FOLLY_EXAMPLE_H
