import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import vm from "node:vm";
import ts from "typescript";

const source = readFileSync(new URL("../src/components/VideoView.tsx", import.meta.url), "utf8");
const start = source.indexOf("  async function performAnalyze(");
const workflowSource = source.slice(start, source.indexOf("\n  // A prefill", start));
const code = ts.transpileModule(workflowSource, { compilerOptions: { target: ts.ScriptTarget.ES2022 } }).outputText;

test("carousel clips have distinct labels even when their quality is the same", () => {
  const start = source.indexOf("function label(format:");
  const body = source.slice(start, source.indexOf("\n}", start) + 2);
  const context = vm.createContext({});
  vm.runInContext(ts.transpileModule(body, { compilerOptions: { target: ts.ScriptTarget.ES2022 } }).outputText, context);
  assert.equal(vm.runInContext("label({id:'clip-1-720p',height:720})", context), "Clip 1 · 720p");
  assert.equal(vm.runInContext("label({id:'clip-2-720p',height:720})", context), "Clip 2 · 720p");
});

test("X analysis keeps a successful result and sends the exact URL once", async () => {
  const calls: unknown[][] = [];
  const metadata = { formats: [{ videoUrl: "https://cdn.test/clip.mp4" }] };
  const context = vm.createContext({ analysisWorkflow: { current: 0 }, video: {
    analyze: async (...args: unknown[]) => { calls.push(args); return calls.length === 1 ? metadata : null; },
  } });
  vm.runInContext(code, context);
  const url = "https://x.com/user/status/123?token=signed";
  context.url = url;
  assert.equal(await vm.runInContext("performAnalyze(url)", context), metadata);
  assert.equal(calls.length, 1);
  assert.equal(calls[0][0], url);
});

test("a late result cannot replace a newer video analysis", async () => {
  let resolveFirst: (value: unknown) => void = () => {};
  const context = vm.createContext({ analysisWorkflow: { current: 0 }, video: {
    analyze: async (url: string) => url.endsWith("/first")
      ? new Promise((resolve) => { resolveFirst = resolve; }) : { title: "second" },
  } });
  vm.runInContext(code, context);
  const first = vm.runInContext("performAnalyze('https://cdn.test/first')", context);
  const second = await vm.runInContext("performAnalyze('https://cdn.test/second')", context);
  resolveFirst({ title: "first" });
  assert.equal(await first, null);
  assert.equal(second.title, "second");
});
