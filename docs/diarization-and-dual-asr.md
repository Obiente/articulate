# Speaker diarization and a second ASR pass

## Current implementation

Articulate uses Qwen3-ASR 1.7B as its final recognizer. Earshot checks for speech before inference. When the optional, separately downloaded Nemotron 3.5 ASR 0.6B Q8_0 model is available, it produces provisional dictation and call previews without forcing a language. Qwen then recognizes completed utterances, and Nemotron checks completed utterances of at most ten seconds. If Qwen emits CJK or Devanagari characters while Nemotron detects another language and supplies Latin text, the second result can replace it. The verifier's detected language takes precedence over a selected preference to protect code switching. Other disagreements retain Qwen's text. A verifier failure retains Qwen's text, while cancellation stops the operation. Calls keep automatic language detection even when dictation has a fixed language setting. The call setup offers a per-recording preference for ambiguous short words, and automatic Discord calls inherit the saved preference. This is a targeted guard, not a claim that two-model consensus has been calibrated for general accuracy.

SenseVoice supplies optional sound and non-neutral tone tags. The installed model and runtime pass their pinned hashes. Local synthetic `Hello` and laughter clips returned `EMO_UNKNOWN`, so enabling tone detection does not guarantee a tone badge. Another synthetic laughter clip returned a `Laughter` event tag, showing that the event path can work. These synthetic clips do not establish real voice performance. A dedicated emotion model such as emotion2vec+ needs separate packaging, latency testing, and recordings with expected labels before it can replace the current tone source.

The conversation view now presents speaker turns on separate colored lanes using saved row time bounds. Overlap may appear on more than one lane when attribution provides multiple speaker labels. Saved speaker names accept up to eight entries while old four-name sessions still load. These are transcript-turn bounds, not frame-level activity probabilities.

## Model research

| Model | Why consider it | Current decision |
| --- | --- | --- |
| [Qwen3-ASR 1.7B](https://huggingface.co/Qwen/Qwen3-ASR-1.7B) | Broad multilingual support and Articulate's existing local runtime | Keep as default until a representative evaluation shows a better primary |
| [Nemotron 3.5 ASR 0.6B](https://huggingface.co/nvidia/nemotron-3.5-asr-streaming-0.6b) | Different RNN-T architecture, local native GGUF conversion, automatic language detection, fast short-clip inference | Provisional dictation previews and optional second final pass |
| [Parakeet TDT 0.6B v3](https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3) | Efficient native model for 25 European languages | Offline-only in the current native runtime and no Chinese support; benchmark on real samples before considering it for final transcription |
| [Whisper large-v3-turbo](https://huggingface.co/openai/whisper-large-v3-turbo) | Useful multilingual comparison point | Evaluate offline; larger memory and latency costs need measurement |
| [Nemotron 3 Diarization](https://huggingface.co/nvidia/Nemotron-3-Diarization) | Eight-speaker streaming probabilities and overlap | Runtime integration remains open |

Nemotron 3 Diarization is a separate speaker model, not the Nemotron 3.5 ASR verifier. Its published NeMo and Transformers paths accept 16 kHz mono audio and emit eight activity channels. Articulate's native [transcribe.cpp](https://github.com/handy-computer/transcribe.cpp) release currently lists only the four-speaker Sortformer v2.1 diarizer. The [NeMo-Speech.cpp](https://github.com/NVIDIA/NeMo-Speech.cpp) native runtime also currently lists Sortformer v2. A production migration needs a Windows native port or a packaged, benchmarked local worker. Simply pointing the existing GGUF loader at the `.nemo` file will not work.

## Validation before changing the default

Use consented, locally held English, Dutch, and mixed-language audio with reference transcripts. Include very short answers, pauses, typing, music, overlap, names, numbers, and corrections. Measure word error rate, character error rate for languages without spaces, false words per silent minute, writing-system switches, speaker diarization error, time to first preview, final-pass latency, CPU/GPU memory, and missed speech. Score Qwen alone, each candidate alone, and the guarded second pass on the same clips. Keep recordings and score reports outside the repository.

Synthetic Windows TTS smoke clips for “Hello”, “Yes”, and “Can we meet at noon tomorrow?” plus digital silence produced correct text or empty output with both Qwen and Nemotron on one local CPU. A longer synthetic “Testing. Hello. Ah. Ah. Hi.” clip remained in Latin script under Qwen auto-detection and Nemotron auto-detection, so it did not reproduce the reported hallucination. Nemotron processed each incremental prefix faster on that CPU, but omitted both “Ah” fillers that Qwen retained. These clips do not establish a quality improvement for a person's voice. Real multilingual and code-switching recordings are needed before changing the final recognizer.

## Next diarization step

Keep direct Discord participant audio and fresh Discord activity as the preferred attribution sources. For mixed audio, add a backend that owns Nemotron 3's speaker cache across chunks and returns timestamped activity for all eight channels. Convert that activity into Articulate's provisional and committed turns, retaining uncertainty and overlap. The current colored lanes can then display its output without changing the transcript layout.
