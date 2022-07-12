/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is licensed under both the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree and the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree.
 */

// cxx-async/include/example.h

#ifndef CXX_ASYNC_EXAMPLE_H
#define CXX_ASYNC_EXAMPLE_H

#include <cstdint>

// Simple PRNG that can be easily duplicated on the Rust and C++ sides to ensure
// identical output.
class Xorshift {
  uint32_t m_state;
  Xorshift(Xorshift&) = delete;
  void operator=(Xorshift) = delete;

 public:
  // Random, but constant, seed.
  Xorshift() : m_state(0x243f6a88) {}

  inline uint32_t next() {
    uint32_t x = m_state;
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    m_state = x;
    return x;
  }
};

// Can't use a C++ `std::binary_semaphore` here because Apple doesn't support
// it.
class Sem {
  unsigned m_value;
  std::condition_variable m_cond;
  std::mutex m_mutex;

  Sem(const Sem&) = delete;
  Sem& operator=(const Sem&) = delete;

 public:
  Sem() : m_value(0) {}

  void wait() {
    std::unique_lock<std::mutex> guard(m_mutex);
    m_cond.wait(guard, [&] { return m_value > 0; });
    m_value--;
  }

  void signal() {
    std::unique_lock<std::mutex> guard(m_mutex);
    m_value++;
    m_cond.notify_one();
  }
};

#endif // CXX_ASYNC_EXAMPLE_H
