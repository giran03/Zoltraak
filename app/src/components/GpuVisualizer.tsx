import { memo, useEffect, useRef, useState } from "react";
import { getGpuDiagnostics, startGpuWaveform, type GpuDiagnostics } from "../lib/webgpu";
import { Legend, Lamp } from "./Panel";
import { Pause, Play } from "./Icons";

interface GpuVisualizerProps {
  hardwareAcceleration: boolean;
  webGpu: boolean;
  primaryColor?: string;
  backgroundColor?: string;
}

export const GpuVisualizer = memo(function GpuVisualizer({
  hardwareAcceleration,
  webGpu,
  primaryColor = "#38bdf8",
  backgroundColor = "#0b0e14",
}: GpuVisualizerProps) {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const fallbackCanvasRef = useRef<HTMLCanvasElement | null>(null);
  const [backend, setBackend] = useState<GpuDiagnostics["activeBackend"]>("disabled");
  const [diagnostics, setDiagnostics] = useState<GpuDiagnostics | null>(null);
  const [fps, setFps] = useState(60);
  const [frameTimeMs, setFrameTimeMs] = useState(16.6);
  const [isPlaying, setIsPlaying] = useState(true);
  // Scrolled out of view, the waveform is not worth a frame loop.
  const [onScreen, setOnScreen] = useState(true);
  const [speed, setSpeed] = useState<number>(1);
  const controllerRef = useRef<ReturnType<typeof startGpuWaveform> | null>(null);

  // Probe GPU diagnostics whenever hardware toggles change
  useEffect(() => {
    let cancelled = false;
    void getGpuDiagnostics(hardwareAcceleration, webGpu).then((diag) => {
      if (!cancelled) setDiagnostics(diag);
    });
    return () => {
      cancelled = true;
    };
  }, [hardwareAcceleration, webGpu]);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas || typeof IntersectionObserver === "undefined") return;
    const observer = new IntersectionObserver(([entry]) => setOnScreen(entry?.isIntersecting ?? true));
    observer.observe(canvas);
    return () => observer.disconnect();
  }, []);

  // Manage WebGPU canvas loop
  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;

    if (!hardwareAcceleration || !isPlaying || !onScreen) {
      setFps(0);
      setFrameTimeMs(0);
      if (controllerRef.current) {
        controllerRef.current.stop();
        controllerRef.current = null;
      }
      return;
    }

    const controller = startGpuWaveform(canvas, {
      hardwareAcceleration,
      webGpu,
      primaryColor,
      backgroundColor,
      onFpsUpdate: (newFps, newFrameTime) => {
        setFps(newFps);
        setFrameTimeMs(newFrameTime);
      },
      onBackendChange: setBackend,
      fallbackCanvas: fallbackCanvasRef.current || undefined,
    });
    controller.setSpeed(speed);
    controllerRef.current = controller;

    return () => {
      controller.stop();
      controllerRef.current = null;
    };
  }, [hardwareAcceleration, webGpu, isPlaying, onScreen]);

  useEffect(() => {
    if (controllerRef.current) {
      controllerRef.current.setColors(primaryColor, backgroundColor);
      controllerRef.current.setSpeed(speed);
    }
  }, [primaryColor, backgroundColor, speed]);

  const togglePlayback = () => {
    setIsPlaying((prev) => !prev);
  };

  const cycleSpeed = () => {
    setSpeed((prev) => (prev === 1 ? 2 : prev === 2 ? 0.5 : 1));
  };

  const lampFill = !hardwareAcceleration
    ? null
    : backend === "webgpu"
      ? "bg-clear"
      : "bg-caution";

  const statusLabel = !hardwareAcceleration
    ? "VISUAL EFFECTS DISABLED"
    : !isPlaying ? "WAVEFORM PAUSED"
    : backend === "webgpu"
      ? "WEBGPU HARDWARE ACCELERATED (NATIVE WGSL)"
      : backend === "canvas2d" ? "CANVAS 2D WAVEFORM" : "WAVEFORM UNAVAILABLE";

  return (
    <div className="plate flex flex-col overflow-hidden rounded-hw border border-edge p-4 shadow-sm">
      {/* Top Status Bar */}
      <div className="flex flex-wrap items-center justify-between gap-3 border-b border-edge pb-3">
        <div className="flex items-center gap-2.5">
          <Lamp fill={lampFill} />
          <div>
            <div className="flex items-center gap-2">
              <span className="legend text-[11px] font-semibold text-ink">{statusLabel}</span>
              {backend === "webgpu" && fps > 0 && (
                <span className="legend rounded-hw-sm border border-clear/50 bg-clear text-on-signal px-1.5 py-[1px] text-[11px] font-semibold tracking-wide">
                  {fps} FPS
                </span>
              )}
            </div>
            <p className="mt-0.5 text-[11px] text-ink-dim">
              {!hardwareAcceleration ? "Visual effects are disabled. Desktop graphics acceleration is unchanged."
                : backend === "canvas2d" ? "Rendering with Canvas 2D; WebGPU is unavailable or switched off."
                : diagnostics?.description || "Scanning graphics subsystem..."}
            </p>
          </div>
        </div>

        {/* Live Gauges */}
        <div className="flex items-center gap-4">
          <div className="flex items-baseline gap-1">
            <span className="describer text-[14px] font-semibold text-ink">
              {hardwareAcceleration && isPlaying ? fps : 0}
            </span>
            <span className="legend text-[11px] text-ink-dim">FPS</span>
          </div>
          <div className="flex items-baseline gap-1 border-l border-edge pl-3">
            <span className="describer text-[14px] font-semibold text-ink">
              {hardwareAcceleration && isPlaying ? frameTimeMs : 0.0}
            </span>
            <span className="legend text-[11px] text-ink-dim">MS / FRAME</span>
          </div>
        </div>
      </div>

      {/* Embedded Real-time Shader Oscillograph */}
      <div
        className="relative my-3 h-36 w-full overflow-hidden rounded-hw border border-edge-strong transition-colors"
        style={{ backgroundColor }}
      >
        <canvas
          ref={canvasRef}
          className="h-full w-full block"
          style={{ imageRendering: "pixelated" }}
        />
        <canvas ref={fallbackCanvasRef} aria-hidden="true" className="absolute inset-0 h-full w-full" style={{ visibility: "hidden", imageRendering: "pixelated" }} />

        {/* Overlay Badges */}
        <div className="pointer-events-none absolute bottom-2.5 left-2.5 flex items-center gap-2">
          <span className="describer rounded-hw border border-edge-strong bg-face/95 px-2 py-0.5 text-[11px] font-medium text-ink shadow-xs backdrop-blur-sm">
            {backend === "webgpu" ? "WGSL SHADER" : "2D RASTER"}
          </span>
          <span className="describer rounded-hw border border-edge-strong bg-face/95 px-2 py-0.5 text-[11px] font-medium text-ink shadow-xs backdrop-blur-sm">
            {speed}x SPEED
          </span>
        </div>

        {/* Floating Controls */}
        <div className="absolute top-2.5 right-2.5 flex items-center gap-1 rounded-hw border border-edge-strong bg-face/95 p-1 shadow-xs backdrop-blur-sm">
          <button
            type="button"
            onClick={togglePlayback}
            disabled={!hardwareAcceleration}
            title={isPlaying ? "Pause visualizer" : "Resume visualizer"}
            className="focus-ring flex size-6 items-center justify-center rounded-hw-sm border border-transparent text-ink transition-colors hover:border-edge hover:bg-recess disabled:opacity-35"
          >
            {isPlaying ? <Pause className="size-3.5" /> : <Play className="size-3.5" />}
          </button>
          <button
            type="button"
            onClick={cycleSpeed}
            disabled={!hardwareAcceleration || !isPlaying}
            title="Cycle simulation frequency"
            className="focus-ring flex h-6 items-center justify-center rounded-hw-sm border border-transparent px-2 text-[11px] font-semibold legend text-ink transition-colors hover:border-edge hover:bg-recess disabled:opacity-35"
          >
            {speed}x
          </button>
        </div>
      </div>

      {/* Hardware Telemetry Specs */}
      <div className="grid grid-cols-2 gap-x-4 gap-y-2 border-t border-edge pt-2.5 sm:grid-cols-4">
        <div>
          <Legend className="text-[11px] text-ink-dim">GRAPHICS ADAPTER</Legend>
          <p className="describer truncate text-[12px] font-medium text-ink mt-0.5" title={diagnostics?.adapterName || "Default GPU"}>
            {diagnostics?.adapterName || "Probing..."}
          </p>
        </div>
        <div>
          <Legend className="text-[11px] text-ink-dim">ARCHITECTURE</Legend>
          <p className="describer truncate text-[12px] font-medium text-ink mt-0.5">
            {diagnostics?.architecture || diagnostics?.vendor || "Discrete / Integrated"}
          </p>
        </div>
        <div>
          <Legend className="text-[11px] text-ink-dim">GRAPHICS DRIVER</Legend>
          <p className="describer truncate text-[12px] font-medium text-ink mt-0.5">
            {diagnostics?.driver || "Direct3D 12 / Vulkan"}
          </p>
        </div>
        <div>
          <Legend className="text-[11px] text-ink-dim">PIPELINE LIMIT</Legend>
          <p className="describer truncate text-[12px] font-medium text-ink mt-0.5">
            {diagnostics?.maxTextureDimension2D ? `${diagnostics.maxTextureDimension2D}px 2D` : "16384px 2D"}
          </p>
        </div>
      </div>
    </div>
  );
});
