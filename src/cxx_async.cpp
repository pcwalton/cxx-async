// cxx-async2/src/cxx_async.cpp

#include "cxx_async.h"
#include <cstdint>
#include "cxx-async2/src/main.rs.h"

namespace cxx {
namespace async {

void cxxasync_assert(bool cond) {
    if (!cond)
        abort();
}

uint8_t* suspended_coroutine_clone(uint8_t* ptr) {
    return reinterpret_cast<uint8_t*>(reinterpret_cast<SuspendedCoroutine*>(ptr)->add_ref());
}

void suspended_coroutine_wake(uint8_t* ptr) {
    suspended_coroutine_wake_by_ref(ptr);
    suspended_coroutine_drop(ptr);
}

void suspended_coroutine_wake_by_ref(uint8_t* ptr) {
    SuspendedCoroutine* coroutine = reinterpret_cast<SuspendedCoroutine*>(ptr);
    if (wake_status_is_done(coroutine->wake()))
        coroutine->resume();
}

void suspended_coroutine_drop(uint8_t* address) {
    reinterpret_cast<SuspendedCoroutine*>(address)->release();
}

}  // namespace async
}  // namespace cxx
