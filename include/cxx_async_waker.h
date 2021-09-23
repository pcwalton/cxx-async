// cxx-async2/include/cxx_async_waker.h

#ifndef CXX_ASYNC2_CXX_ASYNC_WAKER_H
#define CXX_ASYNC2_CXX_ASYNC_WAKER_H

#include <cstdint>

namespace cxx {
namespace async {

uint8_t* suspended_coroutine_clone(uint8_t* ptr);
void suspended_coroutine_wake(uint8_t* ptr);
void suspended_coroutine_wake_by_ref(uint8_t* ptr);
void suspended_coroutine_drop(uint8_t* address);

}  // namespace async
}  // namespace cxx

#endif
