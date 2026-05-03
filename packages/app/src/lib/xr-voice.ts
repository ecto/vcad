/**
 * Push-to-talk voice recognizer for XR (and any other context that wants
 * speech-to-text without a keyboard).
 *
 * Uses the browser's `webkitSpeechRecognition` (the standard in Chromium
 * and Safari, including Vision Pro). Falls back gracefully on browsers
 * that don't expose it — `isVoiceSupported()` returns false and
 * `startVoice()` is a no-op.
 *
 * Single global recognizer because the WebSpeech API is single-instance —
 * starting two simultaneously errors. The XR gesture interpreter is the
 * only caller for now.
 */

interface SpeechRecognitionResultLike {
  isFinal: boolean;
  0: { transcript: string };
}
interface SpeechRecognitionEventLike extends Event {
  resultIndex: number;
  results: ArrayLike<SpeechRecognitionResultLike>;
}
interface SpeechRecognitionLike extends EventTarget {
  continuous: boolean;
  interimResults: boolean;
  lang: string;
  start(): void;
  stop(): void;
  abort(): void;
  onresult: ((ev: SpeechRecognitionEventLike) => void) | null;
  onerror: ((ev: Event) => void) | null;
  onend: (() => void) | null;
}
type SpeechRecognitionCtor = new () => SpeechRecognitionLike;

function getCtor(): SpeechRecognitionCtor | null {
  if (typeof window === "undefined") return null;
  const w = window as unknown as {
    SpeechRecognition?: SpeechRecognitionCtor;
    webkitSpeechRecognition?: SpeechRecognitionCtor;
  };
  return w.SpeechRecognition ?? w.webkitSpeechRecognition ?? null;
}

export function isVoiceSupported(): boolean {
  return getCtor() != null;
}

interface VoiceSession {
  /** Stop recognition and finalize. Calls `onTranscript` with the final
   * accumulated transcript if any, then `onEnd`. Idempotent. */
  stop(): void;
}

interface VoiceOptions {
  /** Called with the latest interim or final transcript. */
  onTranscript: (text: string, isFinal: boolean) => void;
  /** Called when the session ends (release or error). */
  onEnd?: () => void;
  /** Called on error with a short reason. */
  onError?: (reason: string) => void;
  /** BCP-47 language tag, default `"en-US"`. */
  lang?: string;
}

/**
 * Start a push-to-talk voice session. The returned handle's `stop()` ends
 * recognition and produces the final transcript.
 *
 * Returns null if speech recognition isn't available in this UA.
 */
export function startVoice(opts: VoiceOptions): VoiceSession | null {
  const Ctor = getCtor();
  if (!Ctor) {
    opts.onError?.("Speech recognition not supported in this browser.");
    return null;
  }
  const recognition = new Ctor();
  recognition.continuous = true;
  recognition.interimResults = true;
  recognition.lang = opts.lang ?? "en-US";
  let stopped = false;
  let lastFinal = "";
  let lastInterim = "";

  recognition.onresult = (ev: SpeechRecognitionEventLike) => {
    let interim = "";
    let final = lastFinal;
    for (let i = ev.resultIndex; i < ev.results.length; i++) {
      const r = ev.results[i];
      if (!r) continue;
      const text = r[0]?.transcript ?? "";
      if (r.isFinal) final += text;
      else interim += text;
    }
    lastFinal = final;
    lastInterim = interim;
    opts.onTranscript((final + interim).trim(), false);
  };
  recognition.onerror = (ev: Event) => {
    const reason = (ev as unknown as { error?: string }).error ?? "unknown";
    opts.onError?.(reason);
  };
  recognition.onend = () => {
    if (stopped) return;
    stopped = true;
    const finalText = (lastFinal + lastInterim).trim();
    if (finalText) opts.onTranscript(finalText, true);
    opts.onEnd?.();
  };

  try {
    recognition.start();
  } catch (err) {
    opts.onError?.((err as Error).message);
    return null;
  }

  return {
    stop() {
      if (stopped) return;
      try {
        recognition.stop();
      } catch {
        // ignore
      }
    },
  };
}
