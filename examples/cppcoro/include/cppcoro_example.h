// cxx-async/examples/cppcoro/include/cppcoro_example.h

#ifndef CXX_ASYNC_CPPCORO_EXAMPLE_H
#define CXX_ASYNC_CPPCORO_EXAMPLE_H

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

} // namespace bar
} // namespace foo

rust::Box<RustFutureF64> cppcoro_dot_product();
double cppcoro_call_rust_dot_product();
double cppcoro_schedule_rust_dot_product();
rust::Box<foo::bar::RustFutureStringNamespaced> cppcoro_get_namespaced_string();
rust::Box<RustFutureF64> cppcoro_not_product();
rust::String cppcoro_call_rust_not_product();
rust::Box<RustFutureString> cppcoro_ping_pong(int i);
rust::Box<RustFutureVoid> cppcoro_complete();
void cppcoro_send_to_dropped_future_go();
rust::Box<RustFutureF64> cppcoro_send_to_dropped_future();
rust::Box<RustStreamString> cppcoro_fizzbuzz();
rust::Box<RustStreamString> cppcoro_indirect_fizzbuzz();
rust::Box<RustStreamString> cppcoro_not_fizzbuzz();
rust::Box<RustFutureVoid> cppcoro_drop_coroutine_wait();
rust::Box<RustFutureVoid> cppcoro_drop_coroutine_signal();

#endif // CXX_ASYNC_CPPCORO_EXAMPLE_H
