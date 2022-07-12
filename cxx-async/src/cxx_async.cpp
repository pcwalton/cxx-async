/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is licensed under both the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree and the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree.
 */

// cxx-async/src/cxx_async.cpp
//
// Glue functions for C++/Rust async interoperability.

#include "rust/cxx_async.h"
#include <cstdint>
#include <cstdlib>

namespace rust {
namespace async {

void cxxasync_assert(
    bool cond,
    const char* message,
    const char* file,
    int line) {
  if (!cond) {
    fprintf(stderr, "assertion failed: %s at %s:%d\n", message, file, line);
    abort();
  }
}

} // namespace async
} // namespace rust

extern "C" uint8_t* cxxasync_suspended_coroutine_clone(uint8_t* ptr) {
  return reinterpret_cast<uint8_t*>(
      reinterpret_cast<rust::async::SuspendedCoroutine*>(ptr)->add_ref());
}

extern "C" void cxxasync_suspended_coroutine_drop(uint8_t* address) {
  reinterpret_cast<rust::async::SuspendedCoroutine*>(address)->release();
}

extern "C" void cxxasync_suspended_coroutine_wake_by_ref(uint8_t* ptr) {
  rust::async::SuspendedCoroutine* coroutine =
      reinterpret_cast<rust::async::SuspendedCoroutine*>(ptr);
  if (wake_status_is_done(coroutine->wake())) {
    coroutine->resume();
  }
}

extern "C" void cxxasync_suspended_coroutine_wake(uint8_t* ptr) {
  cxxasync_suspended_coroutine_wake_by_ref(ptr);
  cxxasync_suspended_coroutine_drop(ptr);
}
