// SPDX-License-Identifier: AGPL-3.0-or-later
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <node_api.h>
#include "adapter.h"
#include <array>

// Resolve stable Node-API symbols from the hosting Node/Electron executable.
// This avoids binding the addon to a particular Electron NODE_MODULE_VERSION.
#define API_LIST(X) \
    X(napi_get_cb_info) X(napi_create_int32) X(napi_create_function) \
    X(napi_set_named_property) X(napi_get_value_bigint_uint64) \
    X(napi_is_array) X(napi_get_array_length) X(napi_get_element) X(napi_create_buffer_copy)
#define DECLARE(name) decltype(&name) f_##name = nullptr;
API_LIST(DECLARE)
#undef DECLARE
namespace {
napi_value number(napi_env env, int value) noexcept {
    napi_value output = nullptr; f_napi_create_int32(env, value, &output); return output;
}
napi_value start(napi_env env, napi_callback_info) noexcept { return number(env, ArticulateAudioStart()); }
napi_value stop(napi_env env, napi_callback_info) noexcept { ArticulateAudioStop(); return number(env, 0); }
napi_value arm(napi_env env, napi_callback_info info) noexcept {
    size_t count = 2; napi_value args[2]{}; uint64_t capture = 0; bool lossless = false, array = false;
    uint32_t length = 0; std::array<uint64_t, 256> users{};
    if (f_napi_get_cb_info(env, info, &count, args, nullptr, nullptr) != napi_ok || count != 2
        || f_napi_get_value_bigint_uint64(env, args[0], &capture, &lossless) != napi_ok || !lossless
        || f_napi_is_array(env, args[1], &array) != napi_ok || !array
        || f_napi_get_array_length(env, args[1], &length) != napi_ok || length > users.size()) {
        ArticulateAudioStop(); return number(env, 1);
    }
    for (uint32_t i = 0; i < length; ++i) {
        napi_value value;
        if (f_napi_get_element(env, args[1], i, &value) != napi_ok
            || f_napi_get_value_bigint_uint64(env, value, &users[i], &lossless) != napi_ok || !lossless || !users[i]) {
            ArticulateAudioStop(); return number(env, 1);
        }
    }
    return number(env, ArticulateAudioArm(capture, users.data(), length));
}
napi_value poll(napi_env env, napi_callback_info) noexcept {
    std::array<uint8_t, 23104> packet{};
    const auto size = ArticulateAudioPoll(packet.data(), static_cast<uint32_t>(packet.size()));
    napi_value value = nullptr;
    f_napi_create_buffer_copy(env, size, packet.data(), nullptr, &value);
    SecureZeroMemory(packet.data(), packet.size());
    return value;
}
}
extern "C" __declspec(dllexport) int32_t node_api_module_get_api_version_v1() { return 8; }
extern "C" __declspec(dllexport) napi_value napi_register_module_v1(napi_env env, napi_value exports) {
    const HMODULE modules[] = {GetModuleHandleW(nullptr), GetModuleHandleW(L"node.dll")};
#define RESOLVE(name) \
    for (auto module : modules) { if (module) f_##name = reinterpret_cast<decltype(f_##name)>(GetProcAddress(module, #name)); if (f_##name) break; } \
    if (!f_##name) return exports;
    API_LIST(RESOLVE)
#undef RESOLVE
    struct Method { const char* name; napi_callback callback; };
    for (const auto method : {Method{"start", start}, Method{"arm", arm}, Method{"poll", poll}, Method{"stop", stop}}) {
        napi_value function;
        if (f_napi_create_function(env, method.name, NAPI_AUTO_LENGTH, method.callback, nullptr, &function) != napi_ok
            || f_napi_set_named_property(env, exports, method.name, function) != napi_ok) return exports;
    }
    return exports;
}
