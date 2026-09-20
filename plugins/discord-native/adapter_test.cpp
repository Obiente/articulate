// SPDX-License-Identifier: AGPL-3.0-or-later
#include "adapter.h"
#ifdef NDEBUG
#undef NDEBUG
#endif
#include <array>
#include <atomic>
#include <chrono>
#include <cassert>
#include <cstring>
#include <functional>
#include <iostream>
#include <memory>
#include <string>
#include <thread>
#include <vector>

using Received = std::function<void(const std::string&, const int16_t*, uint64_t,
    int, uint64_t, uint32_t, bool&, float)>;
struct Options { int value = 73; };
class Fixture {
public:
    Received received;
    __declspec(noinline) std::shared_ptr<int> connect(const std::string& user,
        const Options& options, std::function<void()> connected, std::function<void()> speaking,
        Received callback, std::function<void()> capture) {
        assert(user == "123" && options.value == 73);
        connected(); speaking(); capture();
        received = std::move(callback);
        return std::make_shared<int>(42);
    }
};
uint64_t get(const uint8_t* p, unsigned n) {
    uint64_t value = 0; for (unsigned i = 0; i < n; ++i) value |= uint64_t(p[i]) << (i * 8); return value;
}
int main() {
    // Production entry point must never install into this ordinary test process.
    assert(ArticulateAudioStart() == 1);
    const auto member = &Fixture::connect;
    void* entry = nullptr; static_assert(sizeof(member) == sizeof(entry));
    std::memcpy(&entry, &member, sizeof(entry));
    assert(ArticulateAudioHookSynthetic(entry) == 0);
    Fixture fixture; Options options;
    int events = 0; std::atomic<int> calls{0};
    const auto result = fixture.connect("123", options, [&] { ++events; }, [&] { ++events; },
        [&](const std::string&, const int16_t*, uint64_t, int, uint64_t, uint32_t opaque, bool&, float gain) {
            assert(opaque == 789 && gain == 0.75f); ++calls;
        }, [&] { ++events; });
    assert(result && *result == 42 && events == 3);
    const uint64_t ids[] = {123, 456};
    assert(ArticulateAudioArm(17, ids, 2) == 0);
    const std::array<int16_t, 4> samples = {100, -100, 200, -200};
    bool muted = false;
    auto emit = [&](const std::string& id = "123", int rate = 48000, uint64_t channels = 2) {
        fixture.received(id, samples.data(), 2, rate, channels, 789, muted, 0.75f);
    };
    emit(); emit("456");
    std::array<uint8_t, 23104> packet{};
    assert(ArticulateAudioPoll(packet.data(), 1) == 0);
    assert(ArticulateAudioPoll(packet.data(), static_cast<uint32_t>(packet.size())) == 72);
    assert(std::memcmp(packet.data(), "APCM", 4) == 0 && get(packet.data() + 4, 2) == 1);
    assert(get(packet.data() + 6, 2) == 64 && get(packet.data() + 8, 8) == 1);
    assert(get(packet.data() + 32, 8) == 123 && get(packet.data() + 40, 4) == 48000);
    assert(get(packet.data() + 44, 2) == 2 && get(packet.data() + 48, 4) == 2);
    assert(get(packet.data() + 52, 8) == 17 && get(packet.data() + 60, 4) == 0);
    assert(std::memcmp(packet.data() + 64, samples.data(), 8) == 0);
    assert(ArticulateAudioPoll(packet.data(), static_cast<uint32_t>(packet.size())) == 72);
    assert(get(packet.data() + 32, 8) == 456);
    emit("999"); emit("../bad"); emit("18446744073709551616"); emit("123", 1); emit("123", 48000, 3);
    muted = true; emit(); assert(muted); muted = false;
    assert(ArticulateAudioPoll(packet.data(), static_cast<uint32_t>(packet.size())) == 0);
    for (int i = 0; i < 140; ++i) emit();
    int drained = 0; while (ArticulateAudioPoll(packet.data(), static_cast<uint32_t>(packet.size()))) ++drained;
    assert(drained == 128);
    emit(); assert(ArticulateAudioArm(18, ids, 2) == 0);
    assert(ArticulateAudioPoll(packet.data(), static_cast<uint32_t>(packet.size())) == 0);
    emit(); ArticulateAudioStop(); emit();
    assert(ArticulateAudioPoll(packet.data(), static_cast<uint32_t>(packet.size())) == 0);
    assert(calls == 151);
    assert(samples[0] == 100 && samples[1] == -100 && samples[2] == 200 && samples[3] == -200);
    assert(ArticulateAudioArm(19, ids, 2) == 0);
    std::atomic<int> remaining{4};
    std::vector<std::thread> producers;
    for (int i = 0; i < 4; ++i) producers.emplace_back([&, i] {
        for (int j = 0; j < 2000; ++j) emit(i % 2 ? "123" : "456");
        --remaining;
    });
    uint64_t last_sequence = 0; int iterations = 0;
    while (true) {
        const auto bytes = ArticulateAudioPoll(packet.data(), static_cast<uint32_t>(packet.size()));
        if (bytes) {
            const auto next = get(packet.data() + 16, 8);
            assert(next > last_sequence && get(packet.data() + 52, 8) == 19);
            last_sequence = next;
        } else if (!remaining.load()) break;
        if (++iterations % 20 == 0) assert(ArticulateAudioArm(19, ids, 2) == 0);
        std::this_thread::yield();
    }
    for (auto& thread : producers) thread.join();
    assert(last_sequence > 0);
    assert(ArticulateAudioArm(19, nullptr, 0) == 0);
    assert(ArticulateAudioArm(19, ids, 2) == 0);
    emit(); assert(ArticulateAudioPoll(packet.data(), static_cast<uint32_t>(packet.size())) == 72);
    assert(get(packet.data() + 16, 8) > last_sequence);
    ArticulateAudioStop();
    assert(ArticulateAudioArm(20, ids, 2) == 0);
    std::this_thread::sleep_for(std::chrono::milliseconds(1600));
    emit();
    assert(ArticulateAudioPoll(packet.data(), static_cast<uint32_t>(packet.size())) == 0);
    ArticulateAudioStop();
    std::cout << "Native adapter synthetic ABI, callback preservation, independent participant PCM, bounds, nonce and shutdown passed.\n";
}
