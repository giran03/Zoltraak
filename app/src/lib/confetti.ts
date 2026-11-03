// Extension-qualified: `node --test` runs these modules unbundled, and its resolver will not guess one.
import { readAppearance } from "./useAppearance.ts";

interface Particle {
  x: number;
  y: number;
  vx: number;
  vy: number;
  size: number;
  color: string;
  shape: "rect" | "circle" | "strip";
  rotation: number;
  rotationSpeed: number;
  wobble: number;
  wobbleSpeed: number;
  opacity: number;
  gravity: number;
  terminalVelocity: number;
}

const COLORS = [
  "#10b981", // Emerald clear
  "#3b82f6", // Route blue
  "#f59e0b", // Amber caution
  "#ec4899", // Vivid pink
  "#8b5cf6", // Purple
  "#06b6d4", // Cyan
  "#f43f5e", // Rose
  "#eab308", // Yellow brass
  "#22c55e", // Bright green
  "#a855f7", // Violet
  "#38bdf8", // Sky blue
];

let activeCanvas: HTMLCanvasElement | null = null;
let animationFrameId: number | null = null;
let particles: Particle[] = [];
let lastConfettiTriggerTime = 0;

/** Cooldown period (in ms) to prevent overlapping confetti storms and sound cacophony */
const CONFETTI_COOLDOWN_MS = 2500;

function createFallingParticle(screenWidth: number, staggerYOffset: number): Particle {
  const shapes: Array<Particle["shape"]> = ["rect", "circle", "strip"];
  const shape = shapes[Math.floor(Math.random() * shapes.length)] ?? "rect";
  const color = COLORS[Math.floor(Math.random() * COLORS.length)] ?? "#10b981";

  // Distribute uniformly across the full screen width with margin padding
  const x = Math.random() * screenWidth;
  // Spawn above the screen, staggered so it falls as a continuous rain curtain
  const y = -20 - Math.random() * 80 - staggerYOffset;

  return {
    x,
    y,
    vx: (Math.random() - 0.5) * 2.2, // Subtle horizontal drift
    vy: 2.2 + Math.random() * 2.8, // Initial downward fall speed
    size: shape === "strip" ? 7 + Math.random() * 7 : 5 + Math.random() * 6,
    color,
    shape,
    rotation: Math.random() * 360,
    rotationSpeed: (Math.random() - 0.5) * 8,
    wobble: Math.random() * Math.PI * 2,
    wobbleSpeed: 0.05 + Math.random() * 0.07,
    opacity: 1,
    gravity: 0.08 + Math.random() * 0.06,
    terminalVelocity: 4.2 + Math.random() * 2.2, // Paper flutter drag
  };
}

function spawnTopRain(screenWidth: number, count = 160) {
  for (let i = 0; i < count; i++) {
    // Stagger particles in depth across 0 to 450px above the viewport
    const stagger = (i / count) * 450 + Math.random() * 60;
    particles.push(createFallingParticle(screenWidth, stagger));
  }
}

function getCanvas(): HTMLCanvasElement {
  if (activeCanvas && document.body.contains(activeCanvas)) {
    return activeCanvas;
  }

  const canvas = document.createElement("canvas");
  canvas.id = "confetti-overlay";
  canvas.style.position = "fixed";
  canvas.style.inset = "0";
  canvas.style.width = "100vw";
  canvas.style.height = "100vh";
  canvas.style.pointerEvents = "none";
  canvas.style.zIndex = "99999";
  document.body.appendChild(canvas);

  const resize = () => {
    const dpr = window.devicePixelRatio || 1;
    canvas.width = window.innerWidth * dpr;
    canvas.height = window.innerHeight * dpr;
    const ctx = canvas.getContext("2d");
    if (ctx) {
      ctx.scale(dpr, dpr);
    }
  };

  resize();
  window.addEventListener("resize", resize, { passive: true });
  activeCanvas = canvas;
  return canvas;
}

function renderLoop() {
  if (!activeCanvas) return;
  const ctx = activeCanvas.getContext("2d");
  if (!ctx) return;

  const width = window.innerWidth;
  const height = window.innerHeight;

  ctx.clearRect(0, 0, width, height);

  for (let i = particles.length - 1; i >= 0; i--) {
    const p = particles[i];
    if (!p) continue;

    // Apply gravity up to terminal flutter velocity
    p.vy = Math.min(p.vy + p.gravity, p.terminalVelocity);
    p.y += p.vy;

    // Aerodynamic wind flutter & horizontal sway
    p.wobble += p.wobbleSpeed;
    p.x += p.vx + Math.sin(p.wobble) * 1.5;
    p.rotation += p.rotationSpeed;

    // Soft fadeout only in the final 80px before exiting the bottom screen
    if (p.y > height - 80) {
      p.opacity -= 0.035;
    }

    // Remove when drifted below the bottom viewport edge
    if (p.y > height + 40 || p.opacity <= 0) {
      particles.splice(i, 1);
      continue;
    }

    // Skip drawing if still above the top viewport
    if (p.y < -30) {
      continue;
    }

    ctx.save();
    ctx.translate(p.x, p.y);
    ctx.rotate((p.rotation * Math.PI) / 180);
    // 3D perspective flip illusion
    ctx.scale(Math.cos(p.wobble), Math.sin(p.wobble * 0.7));
    ctx.globalAlpha = Math.max(0, p.opacity);
    ctx.fillStyle = p.color;

    if (p.shape === "circle") {
      ctx.beginPath();
      ctx.arc(0, 0, p.size / 2, 0, Math.PI * 2);
      ctx.fill();
    } else if (p.shape === "strip") {
      ctx.fillRect(-p.size / 2, -p.size * 1.4, p.size, p.size * 2.8);
    } else {
      ctx.fillRect(-p.size / 2, -p.size / 2, p.size, p.size);
    }

    ctx.restore();
  }

  if (particles.length > 0) {
    animationFrameId = requestAnimationFrame(renderLoop);
  } else {
    // All particles have finished; clean up canvas
    if (activeCanvas && activeCanvas.parentNode) {
      activeCanvas.parentNode.removeChild(activeCanvas);
    }
    activeCanvas = null;
    animationFrameId = null;
  }
}

// Cached audio buffer for the authentic confetti-pop-1.wav sound effect
let confettiAudioBuffer: AudioBuffer | null = null;
let isAudioLoading = false;
let lastSfxPlayTime = 0;

async function loadConfettiAudio(ctx: AudioContext): Promise<AudioBuffer | null> {
  if (confettiAudioBuffer) return confettiAudioBuffer;
  if (isAudioLoading) return null;
  isAudioLoading = true;
  try {
    const response = await fetch("/sfx/confetti-pop-1.wav");
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    const arrayBuffer = await response.arrayBuffer();
    const decoded = await ctx.decodeAudioData(arrayBuffer);
    confettiAudioBuffer = decoded;
    return decoded;
  } catch {
    // Fallback to synthesized audio if file loading fails
    return null;
  } finally {
    isAudioLoading = false;
  }
}

/**
 * Plays the celebratory confetti-pop-1 sound effect, with fallback to synthesized chimes.
 */
export function playConfettiSfx(force = false): void {
  if (typeof window === "undefined") return;

  const now = Date.now();
  if (!force && now - lastSfxPlayTime < CONFETTI_COOLDOWN_MS) {
    return;
  }
  lastSfxPlayTime = now;

  const AudioCtxClass =
    window.AudioContext ||
    (window as unknown as { webkitAudioContext: typeof AudioContext }).webkitAudioContext;
  if (!AudioCtxClass) return;

  try {
    const ctx = new AudioCtxClass();
    if (ctx.state === "suspended") {
      void ctx.resume();
    }

    const now = ctx.currentTime;
    const masterGain = ctx.createGain();
    masterGain.gain.setValueAtTime(0.75, now);
    masterGain.connect(ctx.destination);

    // If pre-loaded or available, play the recorded WAV audio file
    if (confettiAudioBuffer) {
      const source = ctx.createBufferSource();
      source.buffer = confettiAudioBuffer;
      source.connect(masterGain);
      source.start(now);
      return;
    }

    // Try asynchronous load for the next time, and attempt playing now if fast
    void loadConfettiAudio(ctx).then((buf) => {
      if (buf && ctx.state !== "closed") {
        const source = ctx.createBufferSource();
        source.buffer = buf;
        source.connect(masterGain);
        source.start(ctx.currentTime);
      } else {
        // Synthesized fallback if file load failed or took too long
        playSynthesizedFallback(ctx, masterGain, now);
      }
    }).catch(() => {
      playSynthesizedFallback(ctx, masterGain, now);
    });
  } catch {
    // AudioContext permission / autoplay policy handling
  }
}

function playSynthesizedFallback(ctx: AudioContext, masterGain: GainNode, now: number): void {
  // 1. Party popper snappy "pop" impact
  const oscPop = ctx.createOscillator();
  const popGain = ctx.createGain();
  oscPop.type = "triangle";
  oscPop.frequency.setValueAtTime(340, now);
  oscPop.frequency.exponentialRampToValueAtTime(45, now + 0.08);

  popGain.gain.setValueAtTime(0.85, now);
  popGain.gain.exponentialRampToValueAtTime(0.001, now + 0.09);

  oscPop.connect(popGain);
  popGain.connect(masterGain);
  oscPop.start(now);
  oscPop.stop(now + 0.1);

  // 2. Air burst noise puff
  try {
    const bufferSize = Math.floor(ctx.sampleRate * 0.12);
    const buffer = ctx.createBuffer(1, bufferSize, ctx.sampleRate);
    const data = buffer.getChannelData(0);
    for (let i = 0; i < bufferSize; i++) {
      data[i] = (Math.random() * 2 - 1) * Math.exp(-i / (ctx.sampleRate * 0.025));
    }
    const noise = ctx.createBufferSource();
    noise.buffer = buffer;
    const filter = ctx.createBiquadFilter();
    filter.type = "bandpass";
    filter.frequency.setValueAtTime(850, now);
    filter.Q.setValueAtTime(1.5, now);

    const noiseGain = ctx.createGain();
    noiseGain.gain.setValueAtTime(0.38, now);
    noiseGain.gain.exponentialRampToValueAtTime(0.001, now + 0.12);

    noise.connect(filter);
    filter.connect(noiseGain);
    noiseGain.connect(masterGain);
    noise.start(now);
  } catch {
    // Ignore buffer creation errors
  }

  // 3. Celebratory fairy chime arpeggio
  const sparkles = [
    { freq: 880.0, time: 0.03, dur: 0.35 },
    { freq: 1108.73, time: 0.08, dur: 0.4 },
    { freq: 1318.51, time: 0.13, dur: 0.45 },
    { freq: 1760.0, time: 0.18, dur: 0.55 },
    { freq: 2217.46, time: 0.23, dur: 0.65 },
  ];

  for (const { freq, time, dur } of sparkles) {
    const start = now + time;
    const osc = ctx.createOscillator();
    const noteGain = ctx.createGain();

    osc.type = "sine";
    osc.frequency.setValueAtTime(freq, start);

    noteGain.gain.setValueAtTime(0.0001, start);
    noteGain.gain.exponentialRampToValueAtTime(0.32, start + 0.015);
    noteGain.gain.exponentialRampToValueAtTime(0.0001, start + dur);

    osc.connect(noteGain);
    noteGain.connect(masterGain);

    osc.start(start);
    osc.stop(start + dur + 0.05);
  }
}

/**
 * Trigger a full-width top-to-bottom confetti burst cascade across the entire screen.
 * @param force If true, bypasses user appearance setting checks (useful for live preview buttons).
 */
export function triggerConfetti(force = false): void {
  if (typeof window === "undefined" || typeof document === "undefined") {
    return;
  }

  const appearance = readAppearance();

  if (!force) {
    if (appearance.confettiOnComplete === false) {
      return;
    }
    if (appearance.motion === "reduced") {
      return;
    }
    // Prevent overlapping confetti bursts and sound stacking when multiple
    // downloads finish in the same batch or in close succession.
    const now = Date.now();
    if (now - lastConfettiTriggerTime < CONFETTI_COOLDOWN_MS) {
      return;
    }
    lastConfettiTriggerTime = now;
  } else {
    lastConfettiTriggerTime = Date.now();
  }

  // Play celebratory confetti sound effect if enabled
  if (appearance.confettiSfx !== false) {
    playConfettiSfx(force);
  }

  getCanvas();

  const width = window.innerWidth;

  // Spawn a lush shower of 170 particles distributed across the full width of the screen
  spawnTopRain(width, 170);

  // Secondary burst wave after 160ms for a layered, rich celebration rain
  setTimeout(() => {
    if (activeCanvas) {
      spawnTopRain(width, 80);
    }
  }, 160);

  if (!animationFrameId) {
    animationFrameId = requestAnimationFrame(renderLoop);
  }
}
