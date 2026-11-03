/**
 * OpenDM WebGPU Hardware Acceleration & Graphics Subsystem
 *
 * Provides WebGPU capability probing, device enumeration, and a hardware-accelerated
 * phosphor oscillograph / telemetry visualizer with graceful Canvas 2D fallback.
 */

export interface GpuDiagnostics {
  supported: boolean;
  activeBackend: "webgpu" | "canvas2d" | "disabled";
  adapterName?: string;
  vendor?: string;
  architecture?: string;
  driver?: string;
  description?: string;
  maxTextureDimension2D?: number;
  features?: string[];
  error?: string;
}

export interface GpuWaveformOptions {
  hardwareAcceleration: boolean;
  webGpu: boolean;
  primaryColor?: string;
  backgroundColor?: string;
  onFpsUpdate?: (fps: number, frameTimeMs: number) => void;
  onBackendChange?: (backend: GpuDiagnostics["activeBackend"]) => void;
  fallbackCanvas?: HTMLCanvasElement;
}

export interface GpuWaveformController {
  stop: () => void;
  setColors: (primary: string, background: string) => void;
  setSpeed: (speed: number) => void;
  getTelemetry: () => { fps: number; frameTimeMs: number; frameCount: number };
}

/** Check whether WebGPU API is present in the current runtime environment. */
export function isWebGpuSupported(): boolean {
  return typeof navigator !== "undefined" && "gpu" in navigator && Boolean((navigator as any).gpu);
}

/** Query the GPU adapter and gather detailed hardware diagnostics. */
export async function getGpuDiagnostics(
  hardwareAccelerationEnabled = true,
  webGpuEnabled = true,
): Promise<GpuDiagnostics> {
  if (!hardwareAccelerationEnabled) {
    return {
      supported: isWebGpuSupported(),
      activeBackend: "disabled",
      description: "Visual effects and waveform rendering are disabled. Webview acceleration is unchanged.",
    };
  }

  if (!webGpuEnabled) {
    return {
      supported: isWebGpuSupported(),
      activeBackend: "canvas2d",
      description: "WebGPU shaders disabled; utilizing optimized Canvas 2D compositor.",
    };
  }

  if (!isWebGpuSupported()) {
    return {
      supported: false,
      activeBackend: "canvas2d",
      description: "WebGPU is not supported by this browser/webview; falling back to Canvas 2D.",
    };
  }

  try {
    const gpu = (navigator as any).gpu;
    const adapter = await gpu.requestAdapter({ powerPreference: "high-performance" });
    if (!adapter) {
      return {
        supported: true,
        activeBackend: "canvas2d",
        description: "No compatible GPU adapter returned; falling back to Canvas 2D.",
      };
    }

    // Modern browsers expose adapter.info directly or via requestAdapterInfo()
    let adapterInfo: any = {};
    if ("info" in adapter && adapter.info) {
      adapterInfo = adapter.info;
    } else if (typeof (adapter as any).requestAdapterInfo === "function") {
      try {
        adapterInfo = await (adapter as any).requestAdapterInfo();
      } catch {
        // requestAdapterInfo may be restricted
      }
    }

    const featuresList: string[] = [];
    if (adapter.features && typeof adapter.features.forEach === "function") {
      adapter.features.forEach((f: string) => featuresList.push(f));
    }

    return {
      supported: true,
      activeBackend: "webgpu",
      adapterName: adapterInfo.device || adapterInfo.description || adapterInfo.architecture || "Direct3D / Vulkan GPU",
      vendor: adapterInfo.vendor || "Hardware Vendor",
      architecture: adapterInfo.architecture || "Integrated / Discrete GPU",
      driver: adapterInfo.driver || "Direct3D 12 / Vulkan Native",
      description: adapterInfo.description || "Hardware Accelerated WebGPU Engine",
      maxTextureDimension2D: adapter.limits?.maxTextureDimension2D,
      features: featuresList.slice(0, 10),
    };
  } catch (err) {
    return {
      supported: true,
      activeBackend: "canvas2d",
      error: err instanceof Error ? err.message : String(err),
      description: "GPU Adapter initialization failed; falling back to Canvas 2D.",
    };
  }
}

// WGSL Shader for WebGPU Oscillograph Display
const WGSL_SHADER = `
struct Uniforms {
  time: f32,
  width: f32,
  height: f32,
  speed: f32,
  fgColor: vec4<f32>,
  bgColor: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;

struct VertexOutput {
  @builtin(position) position: vec4<f32>,
  @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vertexIndex: u32) -> VertexOutput {
  var pos = array<vec2<f32>, 6>(
    vec2<f32>(-1.0, -1.0),
    vec2<f32>( 1.0, -1.0),
    vec2<f32>(-1.0,  1.0),
    vec2<f32>(-1.0,  1.0),
    vec2<f32>( 1.0, -1.0),
    vec2<f32>( 1.0,  1.0)
  );

  var output: VertexOutput;
  output.position = vec4<f32>(pos[vertexIndex], 0.0, 1.0);
  output.uv = (pos[vertexIndex] + vec2<f32>(1.0, 1.0)) * 0.5;
  return output;
}

@fragment
fn fs_main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
  let aspect = u.width / max(u.height, 1.0);
  let t = u.time * u.speed * 0.002;
  
  // Background gradient with subtle vignette
  let centerDist = length(uv - vec2<f32>(0.5, 0.5));
  var color = mix(u.bgColor, u.bgColor * 0.7, clamp(centerDist * 1.2, 0.0, 1.0));

  // Multi-harmonic sine waveform
  let x = uv.x * 6.283185 * 3.0;
  let wave1 = sin(x + t * 4.0) * 0.18;
  let wave2 = sin(x * 2.3 - t * 2.5) * 0.09;
  let wave3 = sin(x * 5.1 + t * 6.0) * 0.04;
  let targetY = 0.5 + wave1 + wave2 + wave3;

  // Signal beam thickness and glow falloff
  let dist = abs(uv.y - targetY);
  let beamCore = smoothstep(0.006, 0.0005, dist);
  let beamGlow = exp(-dist * 28.0) * 0.65;
  let phosphorBloom = exp(-dist * 10.0) * 0.25;

  let signalIntensity = beamCore + beamGlow + phosphorBloom;
  color = mix(color, u.fgColor, clamp(signalIntensity, 0.0, 1.0));

  // Subtle cathode scanline modulation
  let scanline = sin(uv.y * u.height * 1.5) * 0.04;
  color = color - vec4<f32>(scanline, scanline, scanline, 0.0);

  return clamp(color, vec4<f32>(0.0), vec4<f32>(1.0));
}
`;

function hexToRgbaVec(hex: string): [number, number, number, number] {
  const clean = hex.replace("#", "").trim();
  if (clean.length === 3) {
    const rChar = clean[0] ?? "0";
    const gChar = clean[1] ?? "0";
    const bChar = clean[2] ?? "0";
    const r = parseInt(rChar + rChar, 16) / 255;
    const g = parseInt(gChar + gChar, 16) / 255;
    const b = parseInt(bChar + bChar, 16) / 255;
    return [r, g, b, 1.0];
  }
  if (clean.length >= 6) {
    const r = parseInt(clean.substring(0, 2), 16) / 255;
    const g = parseInt(clean.substring(2, 4), 16) / 255;
    const b = parseInt(clean.substring(4, 6), 16) / 255;
    return [r, g, b, 1.0];
  }
  return [0.35, 0.77, 0.95, 1.0];
}

/**
 * Initializes and starts the WebGPU waveform loop, falling back to Canvas 2D seamlessly.
 */
export function startGpuWaveform(
  canvas: HTMLCanvasElement,
  options: GpuWaveformOptions,
): GpuWaveformController {
  let isStopped = false;
  let animId: number | null = null;
  let fgColor = options.primaryColor || "#38bdf8";
  let bgColor = options.backgroundColor || "#080a0f";
  let speed = 1.0;

  let frameCount = 0;
  let lastTime = performance.now();
  let lastFpsCalcTime = performance.now();
  let currentFps = 60;
  let currentFrameTime = 16.6;

  // WebGPU resources to cleanup
  let gpuDevice: any = null;
  let gpuContext: any = null;
  let renderFrame: ((time: number) => void) | null = null;
  let visible = true;
  let dpr = window.devicePixelRatio || 1;
  let width = 1;
  let height = 1;
  const fallback = options.fallbackCanvas || canvas;
  const measure = () => {
    dpr = window.devicePixelRatio || 1;
    width = Math.max(1, Math.floor(canvas.clientWidth * dpr));
    height = Math.max(1, Math.floor(canvas.clientHeight * dpr));
    for (const target of new Set([canvas, fallback])) {
      if (target.width !== width) target.width = width;
      if (target.height !== height) target.height = height;
    }
  };
  const schedule = () => {
    if (isStopped || !visible || document.hidden || !renderFrame || animId !== null) return;
    animId = requestAnimationFrame((time) => {
      animId = null;
      if (isStopped || !visible || document.hidden) return;
      if (dpr !== (window.devicePixelRatio || 1)) measure();
      renderFrame?.(time);
    });
  };
  const visibilityChanged = () => {
    if (!visible || document.hidden) {
      if (animId !== null) cancelAnimationFrame(animId);
      animId = null;
      currentFps = 0;
      options.onFpsUpdate?.(0, 0);
    } else {
      lastTime = lastFpsCalcTime = performance.now();
      frameCount = 0;
      schedule();
    }
  };
  measure();
  const resizeObserver = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(measure);
  resizeObserver?.observe(canvas);
  const intersectionObserver = typeof IntersectionObserver === "undefined" ? null : new IntersectionObserver(([entry]) => {
    if (!entry) return;
    visible = entry.isIntersecting && entry.intersectionRect.width > 0 && entry.intersectionRect.height > 0;
    visibilityChanged();
  });
  intersectionObserver?.observe(canvas);
  window.addEventListener("resize", measure);
  document.addEventListener("visibilitychange", visibilityChanged);

  const releaseGpu = () => {
    try { gpuContext?.unconfigure(); } catch { /* A lost device may already be unconfigured. */ }
    gpuContext = null;
    const device = gpuDevice;
    gpuDevice = null;
    try { device?.destroy(); } catch { /* Device loss can race teardown. */ }
  };

  const fallbackCanvas2D = () => {
    if (isStopped) return;
    if (animId !== null) cancelAnimationFrame(animId);
    animId = null;
    releaseGpu();
    const ctx = fallback.getContext("2d");
    if (!ctx) { options.onBackendChange?.("disabled"); return; }
    if (fallback !== canvas) fallback.style.visibility = "visible";
    options.onBackendChange?.("canvas2d");

    const render = (time: number) => {
      if (isStopped) return;

      const now = performance.now();
      const delta = now - lastTime;
      lastTime = now;
      frameCount++;

      if (now - lastFpsCalcTime >= 500) {
        currentFps = Math.round((frameCount * 1000) / (now - lastFpsCalcTime));
        currentFrameTime = parseFloat(delta.toFixed(2));
        frameCount = 0;
        lastFpsCalcTime = now;
        options.onFpsUpdate?.(currentFps, currentFrameTime);
      }

      const w = width;
      const h = height;

      // Background
      ctx.fillStyle = bgColor;
      ctx.fillRect(0, 0, w, h);

      // Subtle etched grid lines
      ctx.strokeStyle = "rgba(255, 255, 255, 0.05)";
      ctx.lineWidth = 1;
      const step = 20 * (window.devicePixelRatio || 1);
      ctx.beginPath();
      for (let x = 0; x < w; x += step) {
        ctx.moveTo(x, 0);
        ctx.lineTo(x, h);
      }
      for (let y = 0; y < h; y += step) {
        ctx.moveTo(0, y);
        ctx.lineTo(w, y);
      }
      ctx.stroke();

      // Oscilloscope phosphor wave
      const t = time * speed * 0.002;
      ctx.beginPath();
      ctx.strokeStyle = fgColor;
      ctx.lineWidth = 2.5 * (window.devicePixelRatio || 1);
      ctx.shadowColor = fgColor;
      ctx.shadowBlur = 10 * (window.devicePixelRatio || 1);

      for (let x = 0; x <= w; x += 3) {
        const normX = (x / w) * Math.PI * 6;
        const wave1 = Math.sin(normX + t * 4.0) * (h * 0.18);
        const wave2 = Math.sin(normX * 2.3 - t * 2.5) * (h * 0.09);
        const wave3 = Math.sin(normX * 5.1 + t * 6.0) * (h * 0.04);
        const y = h * 0.5 + wave1 + wave2 + wave3;
        if (x === 0) ctx.moveTo(x, y);
        else ctx.lineTo(x, y);
      }
      ctx.stroke();
      ctx.shadowBlur = 0;

      schedule();
    };

    renderFrame = render;
    schedule();
  };

  const initWebGpu = async () => {
    if (!options.hardwareAcceleration) { options.onBackendChange?.("disabled"); return; }
    if (!options.webGpu || !isWebGpuSupported()) {
      fallbackCanvas2D();
      return;
    }

    try {
      const gpu = (navigator as any).gpu;
      const adapter = await gpu.requestAdapter({ powerPreference: "high-performance" });
      if (isStopped) return;
      if (!adapter) {
        fallbackCanvas2D();
        return;
      }

      const device = await adapter.requestDevice();
      if (isStopped) { device.destroy(); return; }
      gpuDevice = device;
      void device.lost.then(() => { if (!isStopped && gpuDevice === device) fallbackCanvas2D(); });

      const context: any = canvas.getContext("webgpu" as any);
      if (!context) {
        fallbackCanvas2D();
        return;
      }

      const format = gpu.getPreferredCanvasFormat();
      gpuContext = context;
      context.configure({
        device: gpuDevice,
        format,
        alphaMode: "premultiplied",
      });

      const shaderModule = gpuDevice.createShaderModule({
        label: "OpenDM Oscillograph Shader",
        code: WGSL_SHADER,
      });

      const pipeline = gpuDevice.createRenderPipeline({
        label: "OpenDM Oscillograph Pipeline",
        layout: "auto",
        vertex: {
          module: shaderModule,
          entryPoint: "vs_main",
        },
        fragment: {
          module: shaderModule,
          entryPoint: "fs_main",
          targets: [{ format }],
        },
        primitive: {
          topology: "triangle-list",
        },
      });

      // Uniform buffer: UNIFORM (0x40) | COPY_DST (0x08) = 72
      const uniformBufferSize = 64;
      const uniformBuffer = gpuDevice.createBuffer({
        label: "OpenDM Oscillograph Uniforms",
        size: uniformBufferSize,
        usage: 0x0040 | 0x0008,
      });

      const bindGroup = gpuDevice.createBindGroup({
        layout: pipeline.getBindGroupLayout(0),
        entries: [{ binding: 0, resource: { buffer: uniformBuffer } }],
      });

      const uniformData = new Float32Array(16);

      const renderWebGpu = (time: number) => {
        if (isStopped || !gpuDevice) return;

        const now = performance.now();
        const delta = now - lastTime;
        lastTime = now;
        frameCount++;

        if (now - lastFpsCalcTime >= 500) {
          currentFps = Math.round((frameCount * 1000) / (now - lastFpsCalcTime));
          currentFrameTime = parseFloat(delta.toFixed(2));
          frameCount = 0;
          lastFpsCalcTime = now;
          options.onFpsUpdate?.(currentFps, currentFrameTime);
        }

        const w = width;
        const h = height;

        const fg = hexToRgbaVec(fgColor);
        const bg = hexToRgbaVec(bgColor);

        uniformData[0] = time;
        uniformData[1] = w;
        uniformData[2] = h;
        uniformData[3] = speed;
        // fgColor at offset 4
        uniformData[4] = fg[0];
        uniformData[5] = fg[1];
        uniformData[6] = fg[2];
        uniformData[7] = fg[3];
        // bgColor at offset 8
        uniformData[8] = bg[0];
        uniformData[9] = bg[1];
        uniformData[10] = bg[2];
        uniformData[11] = bg[3];

        gpuDevice.queue.writeBuffer(uniformBuffer, 0, uniformData.buffer);

        const commandEncoder = gpuDevice.createCommandEncoder();
        const textureView = context.getCurrentTexture().createView();

        const renderPass = commandEncoder.beginRenderPass({
          colorAttachments: [
            {
              view: textureView,
              clearValue: { r: bg[0], g: bg[1], b: bg[2], a: 1.0 },
              loadOp: "clear",
              storeOp: "store",
            },
          ],
        });

        renderPass.setPipeline(pipeline);
        renderPass.setBindGroup(0, bindGroup);
        renderPass.draw(6);
        renderPass.end();

        gpuDevice.queue.submit([commandEncoder.finish()]);

        schedule();
      };

      if (fallback !== canvas) fallback.style.visibility = "hidden";
      options.onBackendChange?.("webgpu");
      renderFrame = (time) => {
        try { renderWebGpu(time); }
        catch { fallbackCanvas2D(); }
      };
      schedule();
    } catch (err) {
      console.warn("WebGPU initialization failed, falling back to Canvas 2D:", err);
      fallbackCanvas2D();
    }
  };

  void initWebGpu();

  return {
    stop: () => {
      isStopped = true;
      if (animId !== null) {
        cancelAnimationFrame(animId);
        animId = null;
      }
      resizeObserver?.disconnect();
      intersectionObserver?.disconnect();
      window.removeEventListener("resize", measure);
      document.removeEventListener("visibilitychange", visibilityChanged);
      renderFrame = null;
      releaseGpu();
    },
    setColors: (primary: string, background: string) => {
      fgColor = primary;
      bgColor = background;
    },
    setSpeed: (newSpeed: number) => {
      speed = newSpeed;
    },
    getTelemetry: () => ({
      fps: currentFps,
      frameTimeMs: currentFrameTime,
      frameCount,
    }),
  };
}
