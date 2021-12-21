/*
 * Copyright (c) Facebook, Inc. and its affiliates.
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

struct RustFutureVoid;
struct RustFutureF64;
struct RustFutureString;
struct RustStreamString;

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

namespace foo {
namespace bar {

struct RustFutureStringNamespaced;

}
} // namespace foo

rust::Box<RustFutureF64> folly_dot_product_coro();
rust::Box<RustFutureF64> folly_dot_product_futures();
rust::Box<foo::bar::RustFutureStringNamespaced> folly_get_namespaced_string();
double folly_call_rust_dot_product();
double folly_schedule_rust_dot_product();
rust::Box<RustFutureF64> folly_not_product();
rust::String folly_call_rust_not_product();
rust::Box<RustFutureString> folly_ping_pong(int i);
rust::Box<RustFutureVoid> folly_complete();
void folly_send_to_dropped_future_go();
rust::Box<RustFutureF64> folly_send_to_dropped_future();
rust::Box<RustStreamString> folly_fizzbuzz();
rust::Box<RustStreamString> folly_indirect_fizzbuzz();
rust::Box<RustStreamString> folly_not_fizzbuzz();
rust::Box<RustFutureVoid> folly_drop_coroutine_wait();
rust::Box<RustFutureVoid> folly_drop_coroutine_signal();

#endif // CXX_ASYNC_FOLLY_EXAMPLE_H
