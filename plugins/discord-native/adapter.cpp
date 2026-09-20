// SPDX-License-Identifier: AGPL-3.0-or-later
#define WIN32_LEAN_AND_MEAN
#define NOMINMAX
#include <windows.h>
#include <bcrypt.h>
#include <MinHook.h>
#include "adapter.h"
#include <array>
#include <atomic>
#include <cstring>
#include <functional>
#include <string>
#include <utility>

namespace {
using Received = std::function<void(const std::string&, const int16_t*, uint64_t,
    int, uint64_t, uint32_t, bool&, float)>;
static_assert(sizeof(std::string) == 32 && sizeof(Received) == 64,
    "Discord's pinned MSVC ABI uses these layouts");
// Member-function ABI: RCX=this, RDX=shared_ptr return storage, R8=user, R9=options.
// All std::function values are passed indirectly. The original moves them out.
using Connect = void* (__fastcall*)(void*, void*, const std::string*, const void*,
    void*, void*, Received*, void*);
constexpr char connect_symbol[] =
    "?Connect@Discord@@QEAA?AV?$shared_ptr@VConnection@voice@discord@@@std@@AEBV?$basic_string@DU?$char_traits@D@std@@V?$allocator@D@2@@3@AEBUBridgeConnectionOptions@discord@@V?$function@$$A6AXAEBUConnectionInfo@discord@@AEBV?$basic_string@DU?$char_traits@D@std@@V?$allocator@D@2@@std@@@Z@3@V?$function@$$A6AXI@Z@3@V?$function@$$A6AXAEBV?$basic_string@DU?$char_traits@D@std@@V?$allocator@D@2@@std@@PEBF_KH2IAEA_NM@Z@3@V?$function@$$A6AXPEAF_KH1@Z@3@@Z";
constexpr uint8_t module_sha[32] = {
    0x2b,0xf2,0x29,0x0d,0xff,0x75,0x93,0x3c,0x67,0x28,0xbb,0xd8,0xd8,0x0c,0x48,0x76,
    0xd1,0x31,0xa8,0x08,0xf8,0xce,0xa9,0xe1,0x10,0x3e,0x40,0xa1,0x2e,0x64,0xb0,0x14 };
constexpr uint32_t max_samples = 5760 * 2;
constexpr uint32_t max_packet = 64 + max_samples * 2;
struct Frame { uint32_t size = 0; std::array<uint8_t, max_packet> bytes{}; };
std::array<Frame, 128> frames;
std::array<std::atomic<uint64_t>, 256> allowed{};
std::atomic<uint32_t> allowed_count{0};
uint32_t head = 0, tail = 0;
uint64_t capture_id = 0, sequence = 0;
uint32_t capture_epoch = 0;
SRWLOCK queue_lock = SRWLOCK_INIT;
SRWLOCK install_lock = SRWLOCK_INIT;
std::atomic<bool> armed{false};
std::atomic<uint64_t> generation{0}, lease_until{0}, published_capture{0}, pending_drops{0};
Connect original = nullptr;
std::atomic<void*> installed_target{nullptr};

void little(uint8_t* out, uint64_t value, unsigned count) noexcept {
    for (unsigned i = 0; i < count; ++i) out[i] = static_cast<uint8_t>(value >> (i * 8));
}
uint64_t user_id(const std::string& text) noexcept {
    if (text.empty() || text.size() > 20) return 0;
    uint64_t value = 0;
    for (const auto ch : text) {
        if (ch < '0' || ch > '9' || value > (UINT64_MAX - (ch - '0')) / 10) return 0;
        value = value * 10 + (ch - '0');
    }
    return value;
}
void record(uint64_t connection, const std::string& user, const int16_t* data,
    uint64_t samples, int rate, uint64_t channels, bool muted) noexcept {
    if (!armed.load(std::memory_order_relaxed) || GetTickCount64() >= lease_until.load(std::memory_order_relaxed) || muted || !data || !samples
        || samples > 5760 || channels < 1 || channels > 2 || rate < 8000 || rate > 96000) return;
    const auto id = user_id(user);
    if (!id) return;
    const auto observed_capture = published_capture.load(std::memory_order_acquire);
    const auto observed_epoch = pending_drops.load(std::memory_order_acquire) & 0xffffffff00000000ULL;
    bool permitted = false;
    for (uint32_t i = 0, count = allowed_count.load(std::memory_order_relaxed); i < count; ++i)
        permitted |= allowed[i].load(std::memory_order_relaxed) == id;
    if (!permitted) return;
    // Never wait or allocate on Discord's real-time audio thread.
    if (!TryAcquireSRWLockExclusive(&queue_lock)) {
        auto value = pending_drops.load(std::memory_order_relaxed);
        while ((value & 0xffffffff00000000ULL) == observed_epoch && (value & 0xffffffffULL) != 0xffffffffULL) {
            if (pending_drops.compare_exchange_weak(value, value + 1, std::memory_order_relaxed)) break;
        }
        return;
    }
    permitted = false;
    for (uint32_t i = 0; i < allowed_count; ++i) permitted |= allowed[i].load(std::memory_order_relaxed) == id;
    if (!armed.load(std::memory_order_relaxed) || observed_capture != capture_id || !permitted) {
        ReleaseSRWLockExclusive(&queue_lock); return;
    }
    const auto missed = pending_drops.exchange(static_cast<uint64_t>(capture_epoch) << 32, std::memory_order_relaxed) & 0xffffffffULL;
    sequence += 1 + missed;
    const auto seq = sequence;
    if (tail - head == frames.size()) { ReleaseSRWLockExclusive(&queue_lock); return; }
    auto& frame = frames[tail++ % frames.size()];
    auto* p = frame.bytes.data();
    std::memset(p, 0, 64);
    std::memcpy(p, "APCM", 4);
    little(p + 4, 1, 2); little(p + 6, 64, 2);
    little(p + 8, connection, 8); little(p + 16, seq, 8);
    FILETIME time; GetSystemTimePreciseAsFileTime(&time);
    const uint64_t ticks = (static_cast<uint64_t>(time.dwHighDateTime) << 32) | time.dwLowDateTime;
    little(p + 24, (ticks - 116444736000000000ULL) / 10, 8);
    little(p + 32, id, 8); little(p + 40, static_cast<uint32_t>(rate), 4);
    little(p + 44, channels, 2); little(p + 48, samples, 4);
    little(p + 52, capture_id, 8);
    const auto bytes = static_cast<uint32_t>(samples * channels * sizeof(int16_t));
    std::memcpy(p + 64, data, bytes);
    frame.size = 64 + bytes;
    ReleaseSRWLockExclusive(&queue_lock);
}
void* __fastcall connect_hook(void* self, void* result, const std::string* user,
    const void* options, void* connected, void* speaking, Received* receive, void* capture) {
    // Copy before replacing so allocation failure leaves the callback intact.
    // Only connection setup allocates. Existing callbacks remain lifetime owners.
    if (receive) {
        try {
            auto prior = *receive;
            const auto serial = generation.fetch_add(1, std::memory_order_relaxed) + 1;
            Received wrapper = [prior = std::move(prior), serial](const std::string& id,
                const int16_t* samples, uint64_t count, int rate, uint64_t channels,
                uint32_t opaque, bool& muted, float gain) noexcept {
                try {
                    if (prior) prior(id, samples, count, rate, channels, opaque, muted, gain);
                    record(serial, id, samples, count, rate, channels, muted);
                } catch (...) { /* Never unwind through the native audio callback boundary. */ }
            };
            *receive = std::move(wrapper);
        } catch (...) { /* Keep Discord's callback unchanged if wrapping fails. */ }
    }
    return original(self, result, user, options, connected, speaking, receive, capture);
}
bool matching_module(HMODULE module) noexcept {
    wchar_t path[32768];
    const auto length = GetModuleFileNameW(module, path, 32768);
    if (!length || length >= 32768) return false;
    const auto file = CreateFileW(path, GENERIC_READ, FILE_SHARE_READ, nullptr, OPEN_EXISTING,
        FILE_FLAG_SEQUENTIAL_SCAN, nullptr);
    if (file == INVALID_HANDLE_VALUE) return false;
    BCRYPT_ALG_HANDLE algorithm = nullptr; BCRYPT_HASH_HANDLE hash = nullptr;
    bool ok = BCryptOpenAlgorithmProvider(&algorithm, BCRYPT_SHA256_ALGORITHM, nullptr, 0) >= 0
        && BCryptCreateHash(algorithm, &hash, nullptr, 0, nullptr, 0, 0) >= 0;
    std::array<uint8_t, 64 * 1024> buffer{}; DWORD read = 0; uint64_t total = 0;
    while (ok) {
        if (!ReadFile(file, buffer.data(), static_cast<DWORD>(buffer.size()), &read, nullptr)) { ok = false; break; }
        if (!read) break;
        total += read;
        if (total > 128 * 1024 * 1024 || BCryptHashData(hash, buffer.data(), read, 0) < 0) { ok = false; break; }
    }
    std::array<uint8_t, 32> digest{};
    ok = ok && BCryptFinishHash(hash, digest.data(), static_cast<ULONG>(digest.size()), 0) >= 0
        && std::memcmp(digest.data(), module_sha, 32) == 0;
    if (hash) BCryptDestroyHash(hash);
    if (algorithm) BCryptCloseAlgorithmProvider(algorithm, 0);
    CloseHandle(file);
    return ok;
}
int install(void* target) noexcept {
    AcquireSRWLockExclusive(&install_lock);
    if (installed_target) { ReleaseSRWLockExclusive(&install_lock); return installed_target == target ? 0 : 4; }
    const auto init = MH_Initialize();
    if (init != MH_OK && init != MH_ERROR_ALREADY_INITIALIZED) { ReleaseSRWLockExclusive(&install_lock); return 5; }
    auto status = MH_CreateHook(target, reinterpret_cast<void*>(&connect_hook), reinterpret_cast<void**>(&original));
    const bool created = status == MH_OK;
    if (created) status = MH_EnableHook(target);
    if (status != MH_OK) { if (created) MH_RemoveHook(target); original = nullptr; ReleaseSRWLockExclusive(&install_lock); return 6; }
    installed_target = target;
    ReleaseSRWLockExclusive(&install_lock);
    return 0;
}
}

int ArticulateAudioStart() noexcept {
    const auto module = GetModuleHandleW(L"discord_voice.node");
    if (!module) return 1;
    if (!matching_module(module)) return 2;
    auto* target = reinterpret_cast<void*>(GetProcAddress(module, connect_symbol));
    if (!target) return 3;
    const uint8_t prologue[] = {0x41,0x57,0x41,0x56,0x41,0x55,0x41,0x54,0x56,0x57,0x55,0x53};
    if (!installed_target && std::memcmp(target, prologue, sizeof(prologue))) return 3;
    // Wrapped callbacks can outlive a plugin toggle. Keep code mapped until process exit.
    HMODULE self = nullptr;
    if (!GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_PIN,
        reinterpret_cast<LPCWSTR>(&ArticulateAudioStart), &self)) return 7;
    return install(target);
}
int ArticulateAudioArm(uint64_t capture, const uint64_t* ids, uint32_t count) noexcept {
    if (count > allowed.size() || (count && (!ids || !capture)) || !installed_target) return 1;
    AcquireSRWLockExclusive(&queue_lock);
    if (capture_id != capture) {
        armed.store(false, std::memory_order_relaxed);
        SecureZeroMemory(frames.data(), sizeof(frames)); head = tail = 0; sequence = 0;
        pending_drops.store(static_cast<uint64_t>(++capture_epoch) << 32, std::memory_order_release);
    }
    capture_id = capture;
    published_capture.store(capture, std::memory_order_release);
    allowed_count = count;
    for (uint32_t i = 0; i < count; ++i) allowed[i] = ids[i];
    lease_until.store(GetTickCount64() + 1500, std::memory_order_relaxed);
    armed.store(count != 0, std::memory_order_relaxed);
    ReleaseSRWLockExclusive(&queue_lock);
    return 0;
}
uint32_t ArticulateAudioPoll(uint8_t* out, uint32_t capacity) noexcept {
    if (!out) return 0;
    AcquireSRWLockExclusive(&queue_lock);
    uint32_t size = 0;
    if (head != tail) {
        auto& frame = frames[head % frames.size()];
        if (capacity >= frame.size) {
            size = frame.size; std::memcpy(out, frame.bytes.data(), size);
            SecureZeroMemory(frame.bytes.data(), size); frame.size = 0; ++head;
        }
    }
    ReleaseSRWLockExclusive(&queue_lock);
    return size;
}
void ArticulateAudioStop() noexcept {
    armed.store(false, std::memory_order_relaxed);
    published_capture.store(0, std::memory_order_release);
    AcquireSRWLockExclusive(&queue_lock);
    capture_id = 0; allowed_count = 0;
    for (auto& id : allowed) id.store(0, std::memory_order_relaxed);
    SecureZeroMemory(frames.data(), sizeof(frames)); head = tail = 0;
    ReleaseSRWLockExclusive(&queue_lock);
}
#ifdef ARTICULATE_ADAPTER_TEST
int ArticulateAudioHookSynthetic(void* target) noexcept { return install(target); }
#endif
