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
    // A slow poll/control operation must never make the audio callback drop a
    // packet merely because the consumer owns its lock. The old shared-lock
    // queue lost every one of these callbacks.
    assert(ArticulateAudioArm(21, ids, 2) == 0);
    ArticulateAudioHoldConsumerForTest(true);
    for (int i = 0; i < 64; ++i) emit();
    ArticulateAudioHoldConsumerForTest(false);
    for (uint64_t i = 1; i <= 64; ++i) {
        assert(ArticulateAudioPoll(packet.data(), static_cast<uint32_t>(packet.size())) == 72);
        assert(get(packet.data() + 16, 8) == i);
        assert(std::memcmp(packet.data() + 64, samples.data(), 8) == 0);
    }
    // Real capacity exhaustion is still visible as a sequence discontinuity.
    for (int i = 0; i < 140; ++i) emit();
    for (int i = 0; i < 128; ++i) assert(ArticulateAudioPoll(packet.data(), static_cast<uint32_t>(packet.size())) == 72);
    const auto before_gap = get(packet.data() + 16, 8);
    emit(); assert(ArticulateAudioPoll(packet.data(), static_cast<uint32_t>(packet.size())) == 72);
    assert(get(packet.data() + 16, 8) == before_gap + 13);
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
            assert(bytes == 72 && std::memcmp(packet.data() + 64, samples.data(), 8) == 0);
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
    // Repeated stop/rearm while callbacks run never publishes an earlier nonce
    // into the next capture, including slots reserved before the transition.
    std::atomic<bool> run{true};
    std::thread changing_producer([&] { while (run.load()) emit(); });
    for (uint64_t nonce = 30; nonce < 70; ++nonce) {
        ArticulateAudioStop();
        assert(ArticulateAudioArm(nonce, ids, 2) == 0);
        for (int i = 0; i < 20; ++i) {
            const auto bytes = ArticulateAudioPoll(packet.data(), static_cast<uint32_t>(packet.size()));
            if (bytes) assert(get(packet.data() + 52, 8) == nonce);
            std::this_thread::yield();
        }
    }
    run.store(false); changing_producer.join();
    ArticulateAudioStop();
    assert(ArticulateAudioArm(20, ids, 2) == 0);
    std::this_thread::sleep_for(std::chrono::milliseconds(1600));
    emit();
    assert(ArticulateAudioPoll(packet.data(), static_cast<uint32_t>(packet.size())) == 0);
    ArticulateAudioStop();
    // Connect serials identify callback wrappers, not an entire Discord call.
    // An earlier wrapper remains alive while a newer connection emits audio;
    // both share this capture nonce and the queue's ordered packet sequence.
    assert(ArticulateAudioArm(71, ids, 2) == 0);
    auto earlier_callback = fixture.received;
    const auto earlier_calls = calls.load();
    earlier_callback("123", samples.data(), 2, 48000, 2, 789, muted, 0.75f);
    Fixture newer_fixture;
    int newer_calls = 0, newer_events = 0;
    const auto newer_result = newer_fixture.connect("123", options,
        [&] { ++newer_events; }, [&] { ++newer_events; },
        [&](const std::string& id, const int16_t* pcm, uint64_t count, int rate,
            uint64_t channels, uint32_t opaque, bool& should_mute, float gain) {
            assert(id == "456" || id == "123");
            assert(pcm == samples.data() && count == 2 && rate == 48000 && channels == 2);
            assert(opaque == 789 && !should_mute && gain == 0.75f);
            ++newer_calls;
        }, [&] { ++newer_events; });
    assert(newer_result && *newer_result == 42 && newer_events == 3);
    newer_fixture.received("456", samples.data(), 2, 48000, 2, 789, muted, 0.75f);
    earlier_callback("123", samples.data(), 2, 48000, 2, 789, muted, 0.75f);
    newer_fixture.received("456", samples.data(), 2, 48000, 2, 789, muted, 0.75f);
    // A user's old and new callback may also briefly coexist during turnover.
    newer_fixture.received("123", samples.data(), 2, 48000, 2, 789, muted, 0.75f);
    earlier_callback("123", samples.data(), 2, 48000, 2, 789, muted, 0.75f);
    uint64_t earlier_generation = 0, newer_generation = 0;
    const std::array<bool, 6> from_newer = {false, true, false, true, true, false};
    const std::array<uint64_t, 6> participant = {123, 456, 123, 456, 123, 123};
    for (size_t i = 0; i < from_newer.size(); ++i) {
        assert(ArticulateAudioPoll(packet.data(), static_cast<uint32_t>(packet.size())) == 72);
        const auto serial = get(packet.data() + 8, 8);
        if (i == 0) earlier_generation = serial;
        if (i == 1) newer_generation = serial;
        assert(serial == (from_newer[i] ? newer_generation : earlier_generation));
        assert(get(packet.data() + 16, 8) == i + 1);
        assert(get(packet.data() + 32, 8) == participant[i]);
        assert(get(packet.data() + 52, 8) == 71);
        assert(std::memcmp(packet.data() + 64, samples.data(), 8) == 0);
    }
    assert(earlier_generation != 0 && newer_generation > earlier_generation);
    assert(calls == earlier_calls + 3 && newer_calls == 3 && !muted);
    assert(ArticulateAudioPoll(packet.data(), static_cast<uint32_t>(packet.size())) == 0);
    ArticulateAudioStop();
    // A transient localhost failure stops the adapter, then resumes the same
    // Articulate capture nonce. Its delivered sequence must not restart at1.
    assert(ArticulateAudioArm(72, ids, 2) == 0);
    for (uint64_t i = 1; i <= 3; ++i) {
        emit(); assert(ArticulateAudioPoll(packet.data(), static_cast<uint32_t>(packet.size())) == 72);
        assert(get(packet.data() + 16, 8) == i);
    }
    ArticulateAudioStop();
    emit(); assert(ArticulateAudioPoll(packet.data(), static_cast<uint32_t>(packet.size())) == 0);
    ArticulateAudioStop(); // Repeated inactive polls must also retain the sequence.
    assert(ArticulateAudioArm(72, ids, 2) == 0);
    emit(); assert(ArticulateAudioPoll(packet.data(), static_cast<uint32_t>(packet.size())) == 72);
    assert(get(packet.data() + 16, 8) == 4 && get(packet.data() + 52, 8) == 72);
    emit(); emit(); // Queued audio must be erased by Stop, not replayed afterward.
    ArticulateAudioStop();
    assert(ArticulateAudioArm(72, ids, 2) == 0);
    assert(ArticulateAudioPoll(packet.data(), static_cast<uint32_t>(packet.size())) == 0);
    emit(); assert(ArticulateAudioPoll(packet.data(), static_cast<uint32_t>(packet.size())) == 72);
    assert(get(packet.data() + 16, 8) == 7 && get(packet.data() + 52, 8) == 72);
    ArticulateAudioStop();
    assert(ArticulateAudioArm(73, ids, 2) == 0);
    emit(); assert(ArticulateAudioPoll(packet.data(), static_cast<uint32_t>(packet.size())) == 72);
    assert(get(packet.data() + 16, 8) == 1 && get(packet.data() + 52, 8) == 73);
    ArticulateAudioStop();
    auto stats = [] { ArticulateAudioStats value{}; ArticulateAudioGetStats(&value); return value; };
    // Six 10 ms participant streams reproduce one second of a throttled consumer
    // without requiring Discord, a real microphone or network traffic.
    const uint64_t six_ids[] = {123, 456, 789, 890, 901, 912};
    const std::array<std::string, 6> six_users = {"123", "456", "789", "890", "901", "912"};
    std::array<int16_t, 480> ten_ms{};
    for (size_t i = 0; i < ten_ms.size(); ++i) ten_ms[i] = static_cast<int16_t>(static_cast<int>(i) - 240);
    auto paced_load = [&](bool drain) {
        const auto began = std::chrono::steady_clock::now();
        uint64_t read = 0;
        for (int tick = 0; tick < 100; ++tick) {
            for (const auto& user : six_users)
                fixture.received(user, ten_ms.data(), ten_ms.size(), 48000, 1, 789, muted, 0.75f);
            if (drain && tick % 2 == 1) {
                while (const auto bytes = ArticulateAudioPoll(packet.data(), static_cast<uint32_t>(packet.size()))) {
                    ++read;
                    assert(bytes == 1024 && get(packet.data() + 16, 8) == read);
                    assert(std::memcmp(packet.data() + 64, ten_ms.data(), ten_ms.size() * sizeof(int16_t)) == 0);
                }
            }
            std::this_thread::sleep_until(began + std::chrono::milliseconds((tick + 1) * 10));
        }
        return read;
    };
    assert(ArticulateAudioArm(74, six_ids, 6) == 0);
    const auto before_pause = stats();
    assert(paced_load(false) == 0);
    const auto after_pause = stats();
    assert(after_pause.produced - before_pause.produced == 128);
    assert(after_pause.queue_full - before_pause.queue_full == 472);
    assert(after_pause.contention == before_pause.contention);
    assert(after_pause.queue_depth == 128 && after_pause.max_depth == 128 && after_pause.queue_capacity == 128);
    ArticulateAudioStop();
    const auto after_flush = stats();
    assert(after_flush.stop_flush - after_pause.stop_flush == 128 && after_flush.queue_depth == 0);
    assert(ArticulateAudioArm(75, six_ids, 6) == 0);
    const auto before_regular = stats();
    assert(paced_load(true) == 600);
    const auto after_regular = stats();
    assert(after_regular.produced - before_regular.produced == 600);
    assert(after_regular.polled - before_regular.polled == 600);
    assert(after_regular.queue_full == before_regular.queue_full && after_regular.contention == before_regular.contention);
    assert(after_regular.configuration_discard == before_regular.configuration_discard && after_regular.queue_depth == 0);
    emit(); emit();
    const auto before_new_nonce = stats();
    assert(ArticulateAudioArm(76, ids, 2) == 0);
    assert(stats().configuration_discard - before_new_nonce.configuration_discard == 2);
    ArticulateAudioStop();
    std::cout << "Paced 6 x 100 fps: 1 s paused consumer lost 472/600 at capacity 128; 20 ms drain lost 0/600.\n";
    std::cout << "Native adapter synthetic ABI, callback preservation, independent participant PCM, bounds, nonce and shutdown passed.\n";
}
