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
// Exact Discord voice builds whose Connect export and callback ABI were checked.
constexpr std::array<std::array<uint8_t, 32>, 2> module_shas{{
    {0x2b,0xf2,0x29,0x0d,0xff,0x75,0x93,0x3c,0x67,0x28,0xbb,0xd8,0xd8,0x0c,0x48,0x76,
     0xd1,0x31,0xa8,0x08,0xf8,0xce,0xa9,0xe1,0x10,0x3e,0x40,0xa1,0x2e,0x64,0xb0,0x14},
    {0x69,0xe9,0xb8,0x52,0xc2,0x53,0x48,0x7c,0x7e,0xaf,0xa8,0xf6,0x2f,0x12,0x89,0x5a,
     0x00,0xc3,0x55,0x9c,0x57,0x03,0x7e,0xb2,0xfa,0x10,0x3e,0x43,0x76,0x0f,0xd1,0x6d}
}};
constexpr uint32_t max_samples = 5760 * 2;
constexpr uint32_t max_packet = 64 + max_samples * 2;
// A bounded multi-producer/single-consumer ring. Only a slot's owner touches
// its payload; release/acquire publication replaces a shared audio-thread lock.
struct Frame {
    std::atomic<uint32_t> turn{0};
    uint32_t size = 0;
    std::array<uint8_t, max_packet> bytes{};
};
struct Ring {
    std::array<Frame, 128> frames;
    Ring() noexcept { for (uint32_t i = 0; i < frames.size(); ++i) frames[i].turn.store(i); }
} ring;
// Low32: reserved ring positions, high32: dropped callbacks. Updating both in
// one atomic gives each reserved slot a strictly ordered delivery sequence.
std::atomic<uint64_t> producer_state{0}, capture_baseline{0}, configuration{0};
std::atomic<uint64_t> produced{0}, polled{0}, queue_full{0}, contention{0};
std::atomic<uint64_t> configuration_discard{0}, stop_flush{0}, max_depth{0};
std::atomic<uint32_t> consumed_position{0};
static_assert(std::atomic<uint64_t>::is_always_lock_free);
std::atomic<uint64_t> capture_id{0};
std::array<std::atomic<uint64_t>, 256> allowed{};
std::atomic<uint32_t> allowed_count{0};
uint32_t head = 0;
// Stop revokes capture permission, but transport recovery may rearm the SAME
// nonce. Keep its sequence baseline until an actually different capture begins.
// Accessed only while holding control_lock; it never grants audio access.
uint64_t sequence_capture = 0;
// Serializes control and the single consumer, never an audio callback.
SRWLOCK control_lock = SRWLOCK_INIT;
SRWLOCK install_lock = SRWLOCK_INIT;
std::atomic<bool> armed{false};
std::atomic<uint64_t> generation{0}, lease_until{0};
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
// Reserve without waiting for another audio callback or for the UI consumer.
// Contention gets a fixed retry budget; full queues record an explicit gap.
bool reserve(uint32_t& position, uint64_t& ticket) noexcept {
    auto state = producer_state.load(std::memory_order_relaxed);
    for (unsigned attempt = 0; attempt < 16; ++attempt) {
        position = static_cast<uint32_t>(state);
        const auto turn = ring.frames[position % ring.frames.size()].turn.load(std::memory_order_acquire);
        const auto difference = static_cast<int32_t>(turn - position);
        if (difference == 0) {
            const auto next = (state & 0xffffffff00000000ULL) | static_cast<uint32_t>(position + 1);
            if (producer_state.compare_exchange_weak(state, next, std::memory_order_relaxed)) {
                const auto observed_depth = static_cast<uint32_t>(position + 1 - consumed_position.load(std::memory_order_relaxed));
                // Consumer accounting can lag its slot release by a few
                // instructions. The physical ring remains strictly bounded.
                const uint64_t depth = observed_depth > ring.frames.size() ? ring.frames.size() : observed_depth;
                auto peak = max_depth.load(std::memory_order_relaxed);
                for (unsigned i = 0; i < 16 && peak < depth; ++i)
                    if (max_depth.compare_exchange_weak(peak, depth, std::memory_order_relaxed)) break;
                ticket = state; return true;
            }
        } else if (difference < 0) {
            queue_full.fetch_add(1, std::memory_order_relaxed);
            producer_state.fetch_add(1ULL << 32, std::memory_order_relaxed);
            return false;
        }
        else state = producer_state.load(std::memory_order_relaxed);
    }
    contention.fetch_add(1, std::memory_order_relaxed);
    producer_state.fetch_add(1ULL << 32, std::memory_order_relaxed);
    return false;
}
void discard_ready(std::atomic<uint64_t>& discarded) noexcept {
    for (size_t i = 0; i < ring.frames.size(); ++i) {
        auto& frame = ring.frames[head % ring.frames.size()];
        if (frame.turn.load(std::memory_order_acquire) != static_cast<uint32_t>(head + 1)) break;
        if (frame.size) discarded.fetch_add(1, std::memory_order_relaxed);
        SecureZeroMemory(frame.bytes.data(), frame.size); frame.size = 0;
        frame.turn.store(static_cast<uint32_t>(head + ring.frames.size()), std::memory_order_release);
        ++head;
        consumed_position.store(head, std::memory_order_relaxed);
    }
}
void record(uint64_t connection, const std::string& user, const int16_t* data,
    uint64_t samples, int rate, uint64_t channels, bool muted) noexcept {
    const auto version = configuration.load(std::memory_order_acquire);
    if ((version & 1) || !armed.load(std::memory_order_acquire) || GetTickCount64() >= lease_until.load(std::memory_order_relaxed) || muted || !data || !samples
        || samples > 5760 || channels < 1 || channels > 2 || rate < 8000 || rate > 96000) return;
    const auto id = user_id(user);
    if (!id) return;
    const auto nonce = capture_id.load(std::memory_order_relaxed);
    const auto baseline = capture_baseline.load(std::memory_order_relaxed);
    bool permitted = false;
    for (uint32_t i = 0, count = allowed_count.load(std::memory_order_relaxed); i < count; ++i)
        permitted |= allowed[i].load(std::memory_order_relaxed) == id;
    if (!permitted || configuration.load(std::memory_order_acquire) != version) return;
    uint32_t position = 0; uint64_t ticket = 0;
    if (!reserve(position, ticket)) return;
    auto& frame = ring.frames[position % ring.frames.size()];
    auto* p = frame.bytes.data();
    std::memset(p, 0, 64);
    std::memcpy(p, "APCM", 4);
    little(p + 4, 1, 2); little(p + 6, 64, 2);
    const uint64_t seq = static_cast<uint32_t>(position - static_cast<uint32_t>(baseline))
        + static_cast<uint64_t>(static_cast<uint32_t>((ticket >> 32) - (baseline >> 32))) + 1;
    little(p + 8, connection, 8); little(p + 16, seq, 8);
    FILETIME time; GetSystemTimePreciseAsFileTime(&time);
    const uint64_t ticks = (static_cast<uint64_t>(time.dwHighDateTime) << 32) | time.dwLowDateTime;
    little(p + 24, (ticks - 116444736000000000ULL) / 10, 8);
    little(p + 32, id, 8); little(p + 40, static_cast<uint32_t>(rate), 4);
    little(p + 44, channels, 2); little(p + 48, samples, 4);
    little(p + 52, nonce, 8);
    const auto bytes = static_cast<uint32_t>(samples * channels * sizeof(int16_t));
    std::memcpy(p + 64, data, bytes);
    frame.size = 64 + bytes;
    // A stop, nonce or membership change may race the payload copy. Publish an
    // empty slot so the consumer can advance, never audio from an old grant.
    if (configuration.load(std::memory_order_acquire) != version || !armed.load(std::memory_order_acquire)) {
        SecureZeroMemory(p, frame.size); frame.size = 0;
        configuration_discard.fetch_add(1, std::memory_order_relaxed);
    } else produced.fetch_add(1, std::memory_order_relaxed);
    frame.turn.store(static_cast<uint32_t>(position + 1), std::memory_order_release);
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
    ok = ok && BCryptFinishHash(hash, digest.data(), static_cast<ULONG>(digest.size()), 0) >= 0;
    if (ok) {
        ok = false;
        for (const auto& approved_hash : module_shas) {
            if (std::memcmp(digest.data(), approved_hash.data(), digest.size()) == 0) { ok = true; break; }
        }
    }
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
    AcquireSRWLockExclusive(&control_lock);
    bool unchanged = capture_id.load(std::memory_order_relaxed) == capture && allowed_count.load(std::memory_order_relaxed) == count;
    for (uint32_t i = 0; unchanged && i < count; ++i) unchanged = allowed[i].load(std::memory_order_relaxed) == ids[i];
    if (!unchanged) {
        configuration.fetch_add(1, std::memory_order_acq_rel);
        armed.store(false, std::memory_order_release);
        if (sequence_capture != capture) {
            discard_ready(configuration_discard);
            capture_baseline.store(producer_state.load(std::memory_order_relaxed), std::memory_order_relaxed);
            sequence_capture = capture;
        }
        capture_id.store(capture, std::memory_order_relaxed);
        for (uint32_t i = 0; i < count; ++i) allowed[i].store(ids[i], std::memory_order_relaxed);
        allowed_count.store(count, std::memory_order_relaxed);
        configuration.fetch_add(1, std::memory_order_release);
    }
    lease_until.store(GetTickCount64() + 1500, std::memory_order_relaxed);
    armed.store(count != 0, std::memory_order_release);
    ReleaseSRWLockExclusive(&control_lock);
    return 0;
}
uint32_t ArticulateAudioPoll(uint8_t* out, uint32_t capacity) noexcept {
    if (!out) return 0;
    AcquireSRWLockExclusive(&control_lock);
    uint32_t size = 0;
    for (size_t i = 0; i < ring.frames.size(); ++i) {
        auto& frame = ring.frames[head % ring.frames.size()];
        if (frame.turn.load(std::memory_order_acquire) != static_cast<uint32_t>(head + 1)) break;
        uint64_t nonce = 0;
        if (frame.size >= 64) std::memcpy(&nonce, frame.bytes.data() + 52, sizeof(nonce));
        const bool current = frame.size && armed.load(std::memory_order_acquire) && nonce == capture_id.load(std::memory_order_relaxed);
        if (current && capacity < frame.size) break;
        if (current) { size = frame.size; std::memcpy(out, frame.bytes.data(), size); polled.fetch_add(1, std::memory_order_relaxed); }
        else if (frame.size) configuration_discard.fetch_add(1, std::memory_order_relaxed);
        SecureZeroMemory(frame.bytes.data(), frame.size); frame.size = 0;
        frame.turn.store(static_cast<uint32_t>(head + ring.frames.size()), std::memory_order_release);
        ++head;
        consumed_position.store(head, std::memory_order_relaxed);
        if (size) break;
    }
    ReleaseSRWLockExclusive(&control_lock);
    return size;
}
void ArticulateAudioStop() noexcept {
    AcquireSRWLockExclusive(&control_lock);
    configuration.fetch_add(1, std::memory_order_acq_rel);
    armed.store(false, std::memory_order_release);
    capture_id.store(0, std::memory_order_relaxed); allowed_count.store(0, std::memory_order_relaxed);
    for (auto& id : allowed) id.store(0, std::memory_order_relaxed);
    discard_ready(stop_flush);
    configuration.fetch_add(1, std::memory_order_release);
    ReleaseSRWLockExclusive(&control_lock);
}
void ArticulateAudioGetStats(ArticulateAudioStats* out) noexcept {
    if (!out) return;
    // An approximate concurrent snapshot is sufficient; never take the audio or
    // control path's locks just to report diagnostics.
    const auto consumed = consumed_position.load(std::memory_order_relaxed);
    const auto depth = static_cast<uint32_t>(producer_state.load(std::memory_order_relaxed)) - consumed;
    *out = {produced.load(), polled.load(), queue_full.load(), contention.load(),
        configuration_discard.load(), stop_flush.load(), max_depth.load(),
        depth > ring.frames.size() ? ring.frames.size() : depth, ring.frames.size()};
}
#ifdef ARTICULATE_ADAPTER_TEST
int ArticulateAudioHookSynthetic(void* target) noexcept { return install(target); }
void ArticulateAudioHoldConsumerForTest(bool hold) noexcept {
    if (hold) AcquireSRWLockExclusive(&control_lock);
    else ReleaseSRWLockExclusive(&control_lock);
}
#endif
