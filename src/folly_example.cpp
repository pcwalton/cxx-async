// cxx-async2/src/folly_example.cpp

#define FOLLY_HAS_COROUTINES 1

#include "folly_example.h"
#include <folly/experimental/coro/BlockingWait.h>
#include <atomic>
#include <cstdlib>
#include <experimental/coroutine>
#include <functional>
#include <iostream>
#include <optional>
#include "cxx-async2/src/main.rs.h"
#include "cxx_async.h"
#include "rust/cxx.h"

// FIXME(pcwalton): Why do we need this?
namespace folly {

FOLLY_NOINLINE void
resumeCoroutineWithNewAsyncStackRoot(coro::coroutine_handle<> h,
                                     folly::AsyncStackFrame &frame) noexcept {
  detail::ScopedAsyncStackRoot root;
  root.activateFrame(frame);
  h.resume();
}

} // namespace folly

void folly_call_rust_dot_product() {
    rust::Box<RustFutureF64> future = rust_dot_product();
    double result = folly::coro::blockingWait(std::move(future));
    std::cout << result << std::endl;
}
