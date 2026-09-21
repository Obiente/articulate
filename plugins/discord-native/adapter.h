// SPDX-License-Identifier: AGPL-3.0-or-later
#pragma once
#include <cstdint>

// No installation or capture happens at DLL load. A renderer loader must opt in.
// Start returns 0 on success; unsupported versions fail without hooking.
extern "C" __declspec(dllexport) int ArticulateAudioStart() noexcept;
// Arm only current remote participant IDs. Passing no IDs disables capture.
extern "C" __declspec(dllexport) int ArticulateAudioArm(uint64_t capture_id, const uint64_t* ids, uint32_t count) noexcept;
// Copies one APCM packet; zero means empty/too-small destination. Caller owns it.
extern "C" __declspec(dllexport) uint32_t ArticulateAudioPoll(uint8_t* packet, uint32_t capacity) noexcept;
// Stops capture and clears audio. The hook remains dormant until process exit.
extern "C" __declspec(dllexport) void ArticulateAudioStop() noexcept;

// Lifetime aggregate counters only. No audio, participant IDs or capture nonces.
struct ArticulateAudioStats {
    uint64_t produced, polled, queue_full, contention, configuration_discard;
    uint64_t stop_flush, max_depth, queue_depth, queue_capacity;
};
extern "C" __declspec(dllexport) void ArticulateAudioGetStats(ArticulateAudioStats* out) noexcept;

#ifdef ARTICULATE_ADAPTER_TEST
int ArticulateAudioHookSynthetic(void* target) noexcept;
void ArticulateAudioHoldConsumerForTest(bool hold) noexcept;
#endif
