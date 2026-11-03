import { useEffect, useState } from "react";
import { ErrorStrip, Field, Section, SelectInput, Switch, TextInput } from "./Controls";
import { Legend } from "./Panel";
import { SaveIndicator } from "./SaveIndicator";
import type { CompletionAction } from "../lib/engine";
import { useAutosaveSettings } from "../lib/useAutosaveSettings";

function hhmm(minutes: number): string {
  const h = Math.floor(minutes / 60) % 24;
  const m = minutes % 60;
  return `${String(h).padStart(2, "0")}:${String(m).padStart(2, "0")}`;
}

function toMinutes(value: string): number {
  const [h, m] = value.split(":").map(Number);
  return (h || 0) * 60 + (m || 0);
}

type OnCompleteChoice = CompletionAction["type"];

const ON_COMPLETE_OPTIONS: { value: OnCompleteChoice; label: string }[] = [
  { value: "none", label: "Do nothing" },
  { value: "open_folder", label: "Open the default folder" },
  { value: "run_command", label: "Run a command" },
  { value: "exit_app", label: "Exit the app" },
];

/**
 * Start/stop time window plus what to do once the queue drains. Deliberately
 * limited to actions that cannot hurt anything if they fire at the wrong
 * time — open a folder, run a command, exit the app. Sleep, hibernate,
 * shutdown, and VPN disconnect need their own confirmation-gated design and
 * are not here.
 *
 * Commits as you edit, matching the rest of Settings.
 */
export function SchedulerView({ settingsState }: { settingsState: ReturnType<typeof useAutosaveSettings> }) {
  const { settings, loading, error, clearError, commit, saveState } = settingsState;

  const [start, setStart] = useState("22:00");
  const [stop, setStop] = useState("06:00");
  const [command, setCommand] = useState("");

  useEffect(() => {
    if (!settings) return;
    setStart(hhmm(settings.schedule.startMinutes));
    setStop(hhmm(settings.schedule.stopMinutes));
    setCommand(settings.onComplete.type === "run_command" ? settings.onComplete.command : "");
  }, [settings?.schedule.startMinutes, settings?.schedule.stopMinutes, settings?.onComplete]);

  if (loading) {
    return <Legend className="text-ink-dim">reading the timetable…</Legend>;
  }

  if (!settings) {
    return (
      <Section title="Timetable">
        <div className="flex flex-col items-center gap-3 border border-edge bg-face px-6 py-12 text-center">
          <span className="h-px w-24 bg-edge" />
          <Legend className="text-ink-dim">engine detached</Legend>
          <p className="max-w-sm text-[12px] leading-relaxed text-ink-dim">
            The timetable is held by the engine. Launch the desktop app to reach it.
          </p>
        </div>
      </Section>
    );
  }

  const enabled = settings.schedule.enabled;
  const onComplete = settings.onComplete.type;

  const commitSchedule = (patch: Partial<typeof settings.schedule>) =>
    commit({ schedule: { ...settings.schedule, ...patch } });

  const commitAction = (type: OnCompleteChoice, cmd = command) =>
    commit({
      onComplete: type === "run_command" ? { type, command: cmd } : ({ type } as CompletionAction),
    });

  return (
    <div className="flex flex-col gap-7">
      {error && <ErrorStrip message={error} onDismiss={clearError} />}

      <Section
        title="Time window"
        aside={<SaveIndicator state={saveState} />}
        description="Only run downloads between two times of day. Outside the window, downloads in flight are stopped — not abandoned — and pick back up on their own once it reopens. A download you paused by hand stays paused either way."
      >
        <div className="flex items-center justify-between gap-6 border-b border-edge pb-3">
          <p className="text-[13px] text-ink">Run only inside the window</p>
          <Switch
            label="Time window"
            checked={enabled}
            onChange={(next) => void commitSchedule({ enabled: next })}
          />
        </div>

        {enabled && (
          <div className="grid max-w-md grid-cols-2 gap-4">
            <Field label="opens">
              <TextInput
                type="time"
                value={start}
                onChange={setStart}
                onBlur={() => void commitSchedule({ startMinutes: toMinutes(start) })}
                ariaLabel="Window opens at"
              />
            </Field>
            <Field label="closes">
              <TextInput
                type="time"
                value={stop}
                onChange={setStop}
                onBlur={() => void commitSchedule({ stopMinutes: toMinutes(stop) })}
                ariaLabel="Window closes at"
              />
            </Field>
          </div>
        )}
      </Section>

      <Section
        title="When the queue drains"
        description="Sleep, hibernate, shutdown and VPN disconnect are deliberately absent: they are hard to undo if a bug fires them at the wrong moment, and need their own confirmation-gated design."
      >
        <div className="max-w-md">
          <Field label="on completion">
            <SelectInput
              value={onComplete}
              onChange={(next) => void commitAction(next)}
              options={ON_COMPLETE_OPTIONS}
              ariaLabel="When the queue finishes"
            />
          </Field>

          {onComplete === "run_command" && (
            <div className="mt-3">
              <Field label="command">
                <TextInput
                  value={command}
                  onChange={setCommand}
                  onBlur={() => void commitAction("run_command", command)}
                  placeholder="Command to run"
                  mono
                  ariaLabel="Command to run when the queue finishes"
                />
              </Field>
            </div>
          )}
        </div>
      </Section>
    </div>
  );
}
