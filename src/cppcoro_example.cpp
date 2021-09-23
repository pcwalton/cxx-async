// cxx-async2/src/cppcoro_example.cpp

#include <cppcoro/schedule_on.hpp>
#include <cppcoro/static_thread_pool.hpp>
#include <cppcoro/sync_wait.hpp>
#include <cppcoro/task.hpp>
#include <cppcoro/when_all.hpp>
#include <iostream>
#include "cxx-async2/src/main.rs.h"
#include "cxx_async.h"
#include "cxx_async_cppcoro.h"
#include "example.h"
#include "rust/cxx.h"

// HACK
extern "C" {
bool cxxbridge1$string$from_utf8(rust::String *self, const char *ptr,
                                 std::size_t len) noexcept;
}

namespace rust {
inline namespace cxxbridge1 {
static void initString(String *self, const char *s, std::size_t len) {
  if (!cxxbridge1$string$from_utf8(self, s, len))
    throw std::invalid_argument("data for rust::String is not utf-8");
  
}

String::String(const std::string &s) { initString(this, s.data(), s.length()); }

String::operator std::string() const {
    return std::string(this->data(), this->size());
}

}  // namespace cxxbridge1
}  // namespace rust

// Application code follows:

const size_t EXAMPLE_SPLIT_LIMIT = 32;
const size_t EXAMPLE_ARRAY_SIZE = 16384;

cppcoro::static_thread_pool g_thread_pool;

static cppcoro::task<double> dot_product_inner(const double a[], const double b[], size_t count) {
    if (count > EXAMPLE_SPLIT_LIMIT) {
        size_t half_count = count / 2;
        auto [first, second] = co_await cppcoro::when_all(
            cppcoro::schedule_on(g_thread_pool, dot_product_inner(a, b, half_count)),
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

    co_return co_await dot_product_inner(&array_a[0], &array_b[0], array_a.size());
}

rust::Box<RustFutureF64> cppcoro_dot_product() {
    co_return co_await dot_product();
}

void cppcoro_call_rust_dot_product() {
    rust::Box<RustFutureF64> future = rust_dot_product();
    double result = cppcoro::sync_wait(std::move(future));
    std::cout << result << std::endl;
}

rust::Box<RustFutureF64> cppcoro_not_product() {
    if (true)
        throw std::runtime_error("kaboom");
    co_return 1.0;  // just to make this function a coroutine
}

void cppcoro_call_rust_not_product() {
    try {
        rust::Box<RustFutureF64> oneshot_receiver = rust_not_product();
        cppcoro::sync_wait(std::move(oneshot_receiver));
        std::terminate();
    } catch (const std::exception& error) {
        std::cout << error.what() << std::endl;
    }
}

rust::Box<RustFutureString> cppcoro_ping_pong(int i) {
    std::string string(co_await rust_cppcoro_ping_pong(i));
    co_return std::move(string) + "pong ";
}
