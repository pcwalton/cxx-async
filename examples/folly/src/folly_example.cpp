// cxx-async2/examples/folly/src/folly_example.cpp

#define FOLLY_HAS_COROUTINES 1

#include <folly/executors/CPUThreadPoolExecutor.h>
#include <folly/executors/ManualExecutor.h>
#include <folly/experimental/coro/BlockingWait.h>
#include <folly/experimental/coro/Collect.h>
#include <folly/experimental/coro/Coroutine.h>
#include <folly/experimental/coro/Task.h>
#include <folly/tracing/AsyncStack.h>
#include <cstdlib>
#include <iostream>
#include "cxx_async.h"
#include "cxx_async_folly.h"
#include "example.h"
#include "folly/src/main.rs.h"
#include "rust/cxx.h"

// Application code follows:

const size_t EXAMPLE_SPLIT_LIMIT = 32;
const size_t EXAMPLE_ARRAY_SIZE = 16384;

const size_t THREAD_COUNT = 8;

// TODO(pcwalton): It'd be nice to be able to spawn the extra threads that `dot_product` creates on
// a Rust thread pool instead. There has to be some kind of thread pool for this example to actually
// test parallelism, though.
folly::Executor::KeepAlive<folly::CPUThreadPoolExecutor> g_thread_pool(
    new folly::CPUThreadPoolExecutor(THREAD_COUNT));

static folly::coro::Task<double> dot_product_inner(const double a[],
                                                   const double b[],
                                                   size_t count) {
    if (count > EXAMPLE_SPLIT_LIMIT) {
        size_t half_count = count / 2;
        // FIXME(pcwalton): Don't run the second one on the thread pool.
        folly::Future<double> taskA = dot_product_inner(a, b, half_count).semi().via(g_thread_pool);
        folly::Future<double> taskB =
            dot_product_inner(a + half_count, b + half_count, count - half_count)
                .semi()
                .via(g_thread_pool);
        auto [first, second] = co_await folly::collectAll(std::move(taskA), std::move(taskB));
        co_return *first + *second;
    }

    double sum = 0.0;
    for (size_t i = 0; i < count; i++)
        sum += a[i] * b[i];
    co_return sum;
}

static folly::coro::Task<double> dot_product() {
    Xorshift rand;
    std::vector<double> array_a, array_b;
    for (size_t i = 0; i < EXAMPLE_ARRAY_SIZE; i++) {
        array_a.push_back((double)rand.next());
        array_b.push_back((double)rand.next());
    }

    co_return co_await dot_product_inner(&array_a[0], &array_b[0], array_a.size());
}

rust::Box<RustFutureF64> folly_dot_product() {
    co_return co_await dot_product();
}

void folly_call_rust_dot_product() {
    rust::Box<RustFutureF64> future = rust_dot_product();
    double result = folly::coro::blockingWait(std::move(future));
    std::cout << result << std::endl;
}

void folly_schedule_rust_dot_product() {
    rust::Box<RustFutureF64> future = rust_dot_product();
    double result = folly::coro::blockingWait(std::move(future));
    std::cout << result << std::endl;
}

rust::Box<RustFutureF64> folly_not_product() {
    if (true)
        throw std::runtime_error("kaboom");
    co_return 1.0;  // Just to make this function a coroutine.
}

void folly_call_rust_not_product() {
    try {
        rust::Box<RustFutureF64> oneshot_receiver = rust_not_product();
        folly::coro::blockingWait(std::move(oneshot_receiver));
        std::terminate();
    } catch (const std::exception& error) {
        std::cout << error.what() << std::endl;
    }
}

/*
rust::Box<RustFutureString> folly_ping_pong(int i) {
    std::string string(co_await rust_folly_ping_pong(i));
    co_return std::move(string) + "pong ";
}
*/
