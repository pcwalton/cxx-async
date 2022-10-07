/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is licensed under both the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree and the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree.
 */

// cxx-async/examples/cppcoro/src/cppcoro_example.cpp
//
// An example showing how to interoperate with `cppcoro`.

#include "cppcoro_example.h"
#include <cppcoro/async_latch.hpp>
#include <cppcoro/fmap.hpp>
#include <cppcoro/schedule_on.hpp>
#include <cppcoro/static_thread_pool.hpp>
#include <cppcoro/sync_wait.hpp>
#include <cppcoro/task.hpp>
#include <cppcoro/when_all.hpp>
#include <condition_variable>
#include <cstdlib>
#include <exception>
#include <functional>
#include <iosfwd>
#include <iostream>
#include <mutex>
#include <new>
#include <stdexcept>
#include <string>
#include <thread>
#include <tuple>
#include <type_traits>
#include <vector>
#include "cxx-async-example-cppcoro/src/main.rs.h"
#include "example.h"
#include "rust/cxx.h"
#include "rust/cxx_async.h"
#include "rust/cxx_async_cppcoro.h"

const size_t EXAMPLE_SPLIT_LIMIT = 32;
const size_t EXAMPLE_ARRAY_SIZE = 16384;

// TODO(pcwalton): It'd be nice to be able to spawn the extra threads that
// `dot_product` creates on a Rust thread pool instead. There has to be some
// kind of thread pool for this example to actually test parallelism, though.
cppcoro::static_thread_pool g_thread_pool;

// Multithreaded dot product computation.
static cppcoro::task<double>
dot_product_inner(const double a[], const double b[], size_t count) {
  if (count > EXAMPLE_SPLIT_LIMIT) {
    size_t half_count = count / 2;
    auto [first, second] = co_await cppcoro::when_all(
        cppcoro::schedule_on(
            g_thread_pool, dot_product_inner(a, b, half_count)),
        dot_product_inner(a + half_count, b + half_count, count - half_count));
    co_return first + second;
  }

  double sum = 0.0;
  for (size_t i = 0; i < count; i++)
    sum += a[i] * b[i];
  co_return sum;
}

static cppcoro::task<double> dot_product() {
  Xorshift rand;
  std::vector<double> array_a, array_b;
  for (size_t i = 0; i < EXAMPLE_ARRAY_SIZE; i++) {
    array_a.push_back((double)rand.next());
    array_b.push_back((double)rand.next());
  }

  co_return co_await dot_product_inner(
      &array_a[0], &array_b[0], array_a.size());
}

RustFutureF64 cppcoro_dot_product() {
  co_return co_await dot_product();
}

foo::bar::RustFutureStringNamespaced cppcoro_get_namespaced_string() {
  co_return rust::String("hello world");
}

void cppcoro_call_rust_hello() {
  return cppcoro::sync_wait(rust_hello());
}

double cppcoro_call_rust_dot_product() {
  return cppcoro::sync_wait(rust_dot_product());
}

double cppcoro_schedule_rust_dot_product() {
  return cppcoro::sync_wait(
      cppcoro::schedule_on(g_thread_pool, rust_dot_product()));
}

RustFutureF64 cppcoro_not_product() {
  if (true)
    throw MyException("kaboom");
  co_return 1.0; // Just to make this function a coroutine.
}

rust::String cppcoro_call_rust_not_product() {
  try {
    RustFutureF64 oneshot_receiver = rust_not_product();
    cppcoro::sync_wait(std::move(oneshot_receiver));
    std::terminate();
  } catch (const std::exception& error) {
    return rust::String(error.what());
  }
}

RustFutureString cppcoro_ping_pong(int i) {
  std::string string(co_await rust_cppcoro_ping_pong(i));
  co_return std::move(string) + "pong ";
}

RustFutureVoid cppcoro_complete() {
  co_await dot_product(); // Discard the result.
  co_return;
}

// Intentionally leak this to avoid annoying data race issues on thread
// destruction.
static Sem* g_dropped_future_sem;

static cppcoro::task<double> cppcoro_send_to_dropped_future_inner() {
  g_dropped_future_sem->wait();
  co_return 1.0;
}

void cppcoro_send_to_dropped_future_go() {
  g_dropped_future_sem->signal();
}

RustFutureF64 cppcoro_send_to_dropped_future() {
  g_dropped_future_sem = new Sem;
  co_return co_await cppcoro::schedule_on(
      g_thread_pool, cppcoro_send_to_dropped_future_inner());
}

RustStreamString cppcoro_fizzbuzz() {
  for (int i = 1; i <= 15; i++) {
    if (i % 15 == 0) {
      co_yield rust::String("FizzBuzz");
    } else if (i % 5 == 0) {
      co_yield rust::String("Buzz");
    } else if (i % 3 == 0) {
      co_yield rust::String("Fizz");
    } else {
      co_yield rust::String(std::to_string(i));
    }
  }
  co_return;
}

static cppcoro::task<rust::String> fizzbuzz_inner(int i) {
  if (i % 15 == 0)
    co_return rust::String("FizzBuzz");
  if (i % 5 == 0)
    co_return rust::String("Buzz");
  if (i % 3 == 0)
    co_return rust::String("Fizz");
  co_return rust::String(std::to_string(i));
}

RustStreamString cppcoro_indirect_fizzbuzz() {
  for (int i = 1; i <= 15; i++)
    co_yield co_await fizzbuzz_inner(i);
  co_return;
}

RustStreamString cppcoro_not_fizzbuzz() {
  for (int i = 1; i <= 10; i++)
    co_yield co_await fizzbuzz_inner(i);
  throw MyException("kablam");
}

struct DestructorTest {
  cppcoro::async_latch m_latch;
  Sem m_sem;

  DestructorTest() : m_latch(1), m_sem() {}
};

static DestructorTest g_destructor_test;

// Ensure that coroutines run to completion, calling destructors as they do.
// This function, `cppcoro_drop_coroutine_wait()` is called first, and the
// resulting future is dropped. Then `cppcoro_drop_coroutine_signal()` is called
// and should return.
RustFutureVoid cppcoro_drop_coroutine_wait() {
  struct SignalOnDestruction {
    ~SignalOnDestruction() {
      g_destructor_test.m_sem.signal();
    }
  };

  SignalOnDestruction signaller;
  // This makes the coroutine hang until `cppcoro_drop_coroutine_signal()` is
  // called.
  co_await g_destructor_test.m_latch;
  co_return;
}

RustFutureVoid cppcoro_drop_coroutine_signal() {
  // Signal `cppcoro_drop_coroutine_wait()`, which should be running in the
  // background, reparented to the reaper.
  g_destructor_test.m_latch.count_down();
  // Wait for `cppcoro_drop_coroutine_wait()` to finish. The baton is signaled
  // in the destructor of an object on that coroutine's stack.
  g_destructor_test.m_sem.wait();
  co_return;
}
