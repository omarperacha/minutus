import { useEffect, useRef } from "react";

type LaunchSplashProps = {
  onFinished: () => void;
};

export function LaunchSplash({ onFinished }: LaunchSplashProps) {
  const splashRef = useRef<HTMLDivElement>(null);
  const audioRef = useRef<HTMLAudioElement>(null);

  useEffect(() => {
    const splash = splashRef.current;
    const audio = audioRef.current;
    if (!splash || !audio) {
      onFinished();
      return;
    }

    if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) {
      onFinished();
      return;
    }

    let active = true;
    let started = false;
    let soundStarted = false;
    let volumeFrame = 0;
    let playbackTimeout = 0;
    let finishTimeout = 0;
    let objectUrl: string | null = null;
    const abort = new AbortController();

    const syncVolume = () => {
      // Follow the rendered fade instead of maintaining a second audio timeline.
      const opacity = Number.parseFloat(window.getComputedStyle(splash).opacity);
      audio.volume = Number.isFinite(opacity) ? Math.max(0, Math.min(1, opacity)) : 1;
      volumeFrame = window.requestAnimationFrame(syncVolume);
    };

    const start = (withSound: boolean) => {
      if (!active || started) return;
      started = true;
      soundStarted = withSound;
      window.clearTimeout(playbackTimeout);
      splash.dataset.started = "";
      if (withSound) {
        volumeFrame = window.requestAnimationFrame(syncVolume);
      } else {
        abort.abort();
        audio.pause();
      }
      finishTimeout = window.setTimeout(onFinished, 3300);
    };

    const onPlaying = () => {
      if (audio.paused) return;
      if (started && !soundStarted) {
        audio.pause();
        return;
      }
      start(true);
    };

    const onAnimationEnd = (event: AnimationEvent) => {
      if (event.target === splash && event.animationName === "app-launch-exit") {
        onFinished();
      }
    };

    audio.addEventListener("playing", onPlaying);
    splash.addEventListener("animationend", onAnimationEnd);
    audio.volume = 1;
    playbackTimeout = window.setTimeout(() => start(false), 1000);
    void (async () => {
      try {
        // A Blob gives WebKit seekable media without requiring range responses from bundled assets.
        const response = await fetch("/audio/minutus%20intro.wav", { signal: abort.signal });
        if (!response.ok) throw new Error("Launch audio could not be loaded");
        const blob = await response.blob();
        if (!active || started) return;
        objectUrl = URL.createObjectURL(blob);
        audio.src = objectUrl;
        await audio.play();
      } catch {
        start(false);
      }
    })();

    return () => {
      active = false;
      abort.abort();
      window.clearTimeout(playbackTimeout);
      window.clearTimeout(finishTimeout);
      window.cancelAnimationFrame(volumeFrame);
      audio.removeEventListener("playing", onPlaying);
      splash.removeEventListener("animationend", onAnimationEnd);
      audio.volume = 0;
      audio.pause();
      audio.removeAttribute("src");
      audio.load();
      if (objectUrl) URL.revokeObjectURL(objectUrl);
      delete splash.dataset.started;
    };
  }, [onFinished]);

  return (
    <div className="app-launch" aria-hidden="true" ref={splashRef}>
      <audio ref={audioRef} preload="auto" />
      <div className="app-launch-art">
        <svg className="app-launch-mark" viewBox="0 0 260 260" fill="none">
          <defs>
            <linearGradient id="launch-orchid" x1="28" y1="20" x2="222" y2="128" gradientUnits="userSpaceOnUse">
              <stop offset="0%" stopColor="#B975AA" />
              <stop offset="100%" stopColor="#67406E" />
            </linearGradient>
            <mask id="launch-first-stroke" maskUnits="userSpaceOnUse" x="0" y="0" width="260" height="260">
              <path className="app-launch-brush app-launch-brush-first" pathLength="1" d="M 12 108 C 30 78 50 46 74 32 C 95 21 114 43 112 62" stroke="white" strokeWidth="88" strokeLinecap="round" />
            </mask>
            <mask id="launch-second-stroke" maskUnits="userSpaceOnUse" x="0" y="0" width="260" height="260">
              <path className="app-launch-brush app-launch-brush-second" pathLength="1" d="M 91 93 C 107 112 120 100 137 70 C 153 44 168 47 185 73 C 201 98 214 105 229 98 C 241 93 248 85 254 82" stroke="white" strokeWidth="14" strokeLinecap="round" />
            </mask>
          </defs>
          <g transform="translate(0 60)" fill="url(#launch-orchid)">
            <path mask="url(#launch-first-stroke)" d="M15 116C4 117 1 109 4 97C10 72 28 44 45 28C60 14 77 8 93 11C108 13 120 28 130 46C116 58 106 78 97 86C91 91 86 89 83 80C80 70 77 63 72 62C64 61 57 72 50 82L29 109C24 115 19 118 15 116Z" />
            <path mask="url(#launch-second-stroke)" d="M91 93C101 100 110 82 120 65C131 47 145 37 158 35C176 31 189 44 201 61C212 77 219 86 231 84C239 83 241 76 248 76C258 75 258 84 253 94C243 117 224 128 207 125C189 123 178 110 167 94C160 84 155 80 149 84C140 90 134 109 125 118C113 129 101 118 91 93Z" />
          </g>
        </svg>
        <span className="app-launch-wordmark">Minutus</span>
      </div>
    </div>
  );
}
