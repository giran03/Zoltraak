import test from "node:test";
import assert from "node:assert/strict";
import { startGpuWaveform } from "../src/lib/webgpu.ts";

test("waveform retains its buffer, pauses off-screen/hidden, resumes and cleans up", async () => {
  const saved = new Map<string, PropertyDescriptor | undefined>();
  const mock = (name: string, value: unknown) => {
    saved.set(name, Object.getOwnPropertyDescriptor(globalThis, name));
    Object.defineProperty(globalThis, name, { configurable: true, value, writable: true });
  };
  const frames = new Map<number, FrameRequestCallback>();
  const listeners = new Map<string, () => void>();
  let id = 0, dimensionWrites = 0, adapterCalls = 0, destroyed = 0;
  let intersect: (entries: unknown[]) => void = () => {};
  let resized: () => void = () => {};
  const context = new Proxy({}, { get: () => () => {} });
  let width = 0, height = 0;
  const canvas = {
    clientWidth: 320, clientHeight: 120, style: {},
    get width() { return width; }, set width(value) { width = value; dimensionWrites++; },
    get height() { return height; }, set height(value) { height = value; dimensionWrites++; },
    getContext: () => context,
  } as unknown as HTMLCanvasElement;
  const win = { devicePixelRatio: 1, addEventListener: (name: string, fn: () => void) => listeners.set(name, fn), removeEventListener: (name: string) => listeners.delete(name) };
  const doc = { hidden: false, addEventListener: win.addEventListener, removeEventListener: win.removeEventListener };
  const tick = () => { const queued = [...frames.values()]; frames.clear(); queued.forEach(fn => fn(performance.now())); };
  try {
    mock("window", win); mock("document", doc);
    mock("requestAnimationFrame", (fn: FrameRequestCallback) => { frames.set(++id, fn); return id; });
    mock("cancelAnimationFrame", (key: number) => frames.delete(key));
    mock("ResizeObserver", class { constructor(fn: () => void) { resized = fn; } observe() {} disconnect() {} });
    mock("IntersectionObserver", class { constructor(fn: typeof intersect) { intersect = fn; } observe() {} disconnect() {} });
    const controller = startGpuWaveform(canvas, { hardwareAcceleration: true, webGpu: false });
    tick(); tick(); tick();
    assert.equal(dimensionWrites, 2, "unchanged frames must not reset the drawing buffer");
    controller.setColors("#ff0000", "#000000"); controller.setSpeed(2); tick();
    assert.equal(dimensionWrites, 2);
    win.devicePixelRatio = 2; tick();
    assert.equal(canvas.width, 640); assert.equal(canvas.height, 240);
    (canvas as unknown as { clientWidth: number }).clientWidth = 400; resized(); assert.equal(canvas.width, 800);
    intersect([{ isIntersecting: false, intersectionRect: { width: 0, height: 0 } }]);
    assert.equal(frames.size, 0);
    intersect([{ isIntersecting: true, intersectionRect: { width: 400, height: 120 } }]);
    assert.equal(frames.size, 1);
    doc.hidden = true; listeners.get("visibilitychange")!(); assert.equal(frames.size, 0);
    doc.hidden = false; listeners.get("visibilitychange")!(); assert.equal(frames.size, 1);
    controller.stop(); assert.equal(frames.size, 0); assert.equal(listeners.size, 0);

    let resolveDevice: (device: unknown) => void = () => {};
    mock("navigator", { gpu: { requestAdapter: async () => { adapterCalls++; return { requestDevice: () => new Promise(resolve => { resolveDevice = resolve; }) }; } } });
    const pending = startGpuWaveform(canvas, { hardwareAcceleration: true, webGpu: true });
    await Promise.resolve();
    pending.stop(); resolveDevice({ destroy: () => destroyed++ }); await Promise.resolve();
    assert.equal(adapterCalls, 1); assert.equal(destroyed, 1, "device arriving after teardown must be destroyed");
    assert.equal(frames.size, 0); assert.equal(listeners.size, 0);

    let loseDevice: (value: unknown) => void = () => {};
    const backends: string[] = [];
    const gpuContext = { configure() {}, unconfigure() {}, getCurrentTexture: () => ({ createView: () => ({}) }) };
    const device = {
      lost: new Promise(resolve => { loseDevice = resolve; }), destroy: () => destroyed++,
      createShaderModule: () => ({}), createRenderPipeline: () => ({ getBindGroupLayout: () => ({}) }),
      createBuffer: () => ({}), createBindGroup: () => ({}),
      queue: { writeBuffer() {}, submit() {} },
      createCommandEncoder: () => ({ beginRenderPass: () => ({ setPipeline() {}, setBindGroup() {}, draw() {}, end() {} }), finish: () => ({}) }),
    };
    Object.defineProperty(globalThis, "navigator", { configurable: true, value: { gpu: {
      requestAdapter: async () => ({ requestDevice: async () => device }), getPreferredCanvasFormat: () => "bgra8unorm",
    } } });
    canvas.getContext = ((kind: string) => kind === "webgpu" ? gpuContext : context) as typeof canvas.getContext;
    const fallback = { ...canvas, style: {} } as HTMLCanvasElement;
    const accelerated = startGpuWaveform(canvas, { hardwareAcceleration: true, webGpu: true, fallbackCanvas: fallback, onBackendChange: backend => backends.push(backend) });
    await new Promise(resolve => setImmediate(resolve));
    tick(); assert.deepEqual(backends, ["webgpu"]);
    loseDevice({ reason: "unknown" }); await Promise.resolve();
    assert.deepEqual(backends, ["webgpu", "canvas2d"]);
    assert.equal(fallback.style.visibility, "visible");
    tick(); accelerated.stop();
    assert.equal(frames.size, 0); assert.equal(listeners.size, 0);
  } finally {
    for (const [name, descriptor] of saved) {
      if (descriptor) Object.defineProperty(globalThis, name, descriptor);
      else Reflect.deleteProperty(globalThis, name);
    }
  }
});
