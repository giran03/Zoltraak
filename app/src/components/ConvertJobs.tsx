import type { ReactNode } from "react";
import { Button, Field, Section, SelectInput, Switch } from "./Controls";
import { ChevronDown, FolderOpen, Pause, Play, RotateCcw, Trash } from "./Icons";
import { Legend } from "./Panel";
import { engine } from "../lib/engine";
import { compareJob } from "../lib/conversion";
import { basename } from "../lib/media";
import { formatBytes } from "../lib/format";
import type { useConvert } from "../lib/useConvert";

type Converter = ReturnType<typeof useConvert>;
const PARALLEL_CHOICES = [1, 2, 3, 4];

function ConvertIconButton({ label, kind, onClick, disabled, children }: {
  label: string; kind: "folder" | "remove" | "retry" | "cancel" | "next" | "up" | "down";
  onClick: () => void; disabled?: boolean; children: ReactNode;
}) {
  return <button type="button" className="entry-icon-button focus-ring" data-action={kind} aria-label={label} title={label} disabled={disabled} onClick={onClick}><span className="entry-icon-glyph">{children}</span></button>;
}

function QueueControls({ convert }: { convert: Converter }) {
  const { queue, afterBatch } = convert;
  return <details className="border-t border-edge pt-3">
    <summary className="focus-ring cursor-pointer text-[12px] text-ink-dim">Queue and notifications</summary>
    <div className="mt-4 grid gap-4 sm:grid-cols-2">
      <Field label="Conversions at once" hint="More at once finishes a batch sooner but uses more of your computer. Waiting jobs start in order.">
        <SelectInput ariaLabel="Conversions at once" value={String(queue.parallelism)} onChange={(value) => void convert.setParallelism(Number(value))} options={PARALLEL_CHOICES.map((count) => ({ value: String(count), label: String(count) }))} />
      </Field>
      <div className="flex flex-col gap-3">
        <Legend>When a batch finishes</Legend>
        {([["notify", "Show a notification"], ["openFolder", "Show the last result in its folder"], ["copyPath", "Copy result paths to the clipboard"]] as const).map(([key, label]) =>
          <label key={key} className="flex items-center gap-3 text-[13px] text-ink"><Switch checked={afterBatch[key]} label={label} onChange={(value) => convert.setAfterBatch({ ...afterBatch, [key]: value })} />{label}</label>)}
      </div>
    </div>
  </details>;
}

export function ConvertJobs({ convert }: { convert: Converter }) {
  const waiting = convert.queue.queued;
  return <Section title="Conversions" aside={<div className="flex items-center gap-2">
    <Button tone="ghost" onClick={() => void convert.setPaused(!convert.queue.paused)}>{convert.queue.paused ? "Resume queue" : "Pause queue"}</Button>
    <Button tone="ghost" onClick={() => void convert.clearDone()}>Clear finished</Button>
  </div>}>
    {convert.queue.paused && <p role="status" className="text-[12px] text-caution">The queue is paused. Running conversions finish; waiting ones start when you resume.</p>}
    {convert.jobs.length === 0 ? <p className="py-6 text-center text-[13px] text-ink-dim">Conversions started here or from a completed download appear here.</p> :
      <div className="recess rounded-hw-lg">{convert.jobs.map((job) => {
        const active = ["queued", "probing", "running"].includes(job.state);
        const retryable = job.state === "failed" || job.state === "cancelled";
        const place = waiting.indexOf(job.id);
        const change = compareJob(job);
        return <div key={job.id} className="flex items-center gap-3 border-b border-edge px-4 py-3 last:border-b-0">
          <div className="min-w-0 flex-1">
            <div className="flex flex-wrap items-center gap-2"><span className="truncate text-[13px] text-ink" title={job.input}>{job.filename}</span><Legend>{job.conversion?.format ?? job.container}</Legend><Legend className={job.error ? "text-danger" : "text-ink-dim"}>{place >= 0 ? `waiting · ${place + 1} of ${waiting.length}` : job.state}</Legend></div>
            <p data-selectable className={`mt-1 text-[11px] ${job.error ? "text-danger" : "text-ink-dim"}`}>{job.error ?? job.stage}</p>
            {job.output && job.state === "finished" && <p className="mt-1 truncate text-[11px] text-ink-dim" title={job.output}>{basename(job.output)}{job.outputBytes ? ` · ${formatBytes(job.outputBytes)}` : ""}</p>}
            {change && <p data-selectable data-testid="job-comparison" className="mt-1 text-[11px] text-ink-dim">
              {formatBytes(change.inputBytes)} → {formatBytes(change.outputBytes)} · <span className={change.savedPercent >= 0 ? "text-clear" : "text-caution"}>{change.savedPercent >= 0 ? `${change.savedPercent}% smaller` : `${-change.savedPercent}% larger`}</span>
              {change.from && change.to && change.from !== change.to ? ` · ${change.from} → ${change.to}` : ""}
            </p>}
            {job.state === "running" && <progress aria-label={`Converting ${job.filename}`} max={100} value={job.percent > 0 ? job.percent : undefined} className="mt-2 h-1.5 w-full accent-clear" />}
          </div>
          {place > 0 && <ConvertIconButton label={`Run ${job.filename} next`} kind="next" onClick={() => void convert.moveJob(job.id, "top")}><Play className="size-4" /></ConvertIconButton>}
          {place >= 0 && <>
            <ConvertIconButton label={`Move ${job.filename} up`} kind="up" disabled={place === 0} onClick={() => void convert.moveJob(job.id, "up")}><ChevronDown className="size-4 rotate-180" /></ConvertIconButton>
            <ConvertIconButton label={`Move ${job.filename} down`} kind="down" disabled={place === waiting.length - 1} onClick={() => void convert.moveJob(job.id, "down")}><ChevronDown className="size-4" /></ConvertIconButton>
          </>}
          {active && <ConvertIconButton label={`Cancel ${job.filename}`} kind="cancel" onClick={() => void convert.cancel(job.id)}><Pause className="size-4" /></ConvertIconButton>}
          {retryable && <ConvertIconButton label={`Retry ${job.filename}`} kind="retry" onClick={() => void convert.retry(job.id)}><RotateCcw className="size-4" /></ConvertIconButton>}
          {job.output && !active && <ConvertIconButton label={`Reveal ${job.filename}`} kind="folder" onClick={() => void engine.openRemuxOutput(job.id)}><FolderOpen className="size-4" /></ConvertIconButton>}
          <ConvertIconButton label={`Remove ${job.filename} from history`} kind="remove" onClick={() => void convert.remove(job.id)}><Trash className="size-4" /></ConvertIconButton>
        </div>;
      })}</div>}
    <QueueControls convert={convert} />
    <p className="text-[11px] text-ink-faint">History and job settings survive restarts. Retry restarts an interrupted conversion from the beginning.</p>
  </Section>;
}
