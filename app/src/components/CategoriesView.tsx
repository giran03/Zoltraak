import { useState } from "react";
import { Button, ErrorStrip, Field, Section, SelectInput, Switch, TextInput } from "./Controls";
import { FolderOpen, Trash } from "./Icons";
import { Legend } from "./Panel";
import type { CategoryKind } from "../lib/engine";
import { useCategories } from "../lib/useCategories";

const KIND_LABEL: Record<CategoryKind, string> = {
  extension: "Extension",
  domain: "Domain",
  regex: "Regex",
};

const KIND_HINT: Record<CategoryKind, string> = {
  extension: "zip, rar, 7z",
  domain: "github.com",
  regex: "\\.iso(\\?|$)",
};

const KIND_OPTIONS = (Object.keys(KIND_LABEL) as CategoryKind[]).map((kind) => ({
  value: kind,
  label: KIND_LABEL[kind],
}));

function AddCategoryForm({
  onAdd,
}: {
  onAdd: (
    name: string,
    kind: CategoryKind,
    pattern: string,
    directory: string,
  ) => Promise<boolean>;
}) {
  const [name, setName] = useState("");
  const [kind, setKind] = useState<CategoryKind>("extension");
  const [pattern, setPattern] = useState("");
  const [directory, setDirectory] = useState("");
  const [busy, setBusy] = useState(false);

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    if (!name.trim() || !pattern.trim() || !directory.trim() || busy) return;
    setBusy(true);
    const ok = await onAdd(name.trim(), kind, pattern.trim(), directory.trim());
    setBusy(false);
    if (ok) {
      setName("");
      setPattern("");
      setDirectory("");
    }
  }

  return (
    <form
      autoComplete="off" onSubmit={(e) => void submit(e)}
      className="grid grid-cols-1 items-end gap-3 sm:grid-cols-2 lg:grid-cols-[1fr_9rem_1fr_1fr_auto]"
    >
      <Field label="name">
        <TextInput value={name} onChange={setName} placeholder="Archives" ariaLabel="Rule name" />
      </Field>
      <Field label="match on">
        <SelectInput
          value={kind}
          onChange={setKind}
          options={KIND_OPTIONS}
          ariaLabel="Match kind"
        />
      </Field>
      <Field label="pattern">
        <TextInput
          value={pattern}
          onChange={setPattern}
          placeholder={KIND_HINT[kind]}
          mono
          ariaLabel="Match pattern"
        />
      </Field>
      <Field label="route to">
        <TextInput
          value={directory}
          onChange={setDirectory}
          placeholder={"D:\\Downloads\\Archives"}
          ariaLabel="Destination folder"
        />
      </Field>
      <Button type="submit" disabled={busy} className="h-[31px]">
        Add rule
      </Button>
    </form>
  );
}

/**
 * Filing rules: match a download by extension, domain, or regex, and route it
 * to a folder instead of wherever it would otherwise land. Evaluated once, at
 * enqueue time — editing a rule never moves files an earlier match placed.
 */
export function CategoriesView() {
  const categories = useCategories();

  return (
    <div className="mx-auto flex w-full max-w-5xl flex-col gap-7 px-6 py-6">
      <Section
        title="Routing rules"
        aside={
          <Legend>
            {categories.items.length} {categories.items.length === 1 ? "rule" : "rules"}
          </Legend>
        }
        description="Evaluated once, when a download is added — the highest-priority match wins. Nothing here moves a file a rule already placed, and a folder you chose on purpose is never overridden."
      >
        <AddCategoryForm onAdd={categories.add} />
      </Section>

      {categories.error && (
        <ErrorStrip message={categories.error} onDismiss={categories.clearError} />
      )}

      <Section title="In force">
        {!categories.loading && categories.items.length === 0 ? (
          <div className="flex flex-col items-center gap-3 border border-edge bg-face px-6 py-14 text-center">
            <span className="h-px w-24 bg-edge" />
            <Legend className="text-ink-dim">no rules set</Legend>
            <p className="max-w-sm text-[12px] leading-relaxed text-ink-dim">
              Add one above and every download that matches lands in its folder automatically.
            </p>
          </div>
        ) : (
          <div className="border border-edge bg-face">
            <div className="flex items-center gap-3 border-b border-edge bg-recess px-4 py-1">
              <Legend className="flex-[2]">rule</Legend>
              <Legend className="hidden flex-[3] sm:block">pattern</Legend>
              <Legend className="hidden flex-[3] md:block">route to</Legend>
              <span className="w-[104px] shrink-0" />
            </div>

            {categories.items.map((c) => (
              <div
                key={c.id}
                className="flex items-center gap-3 border-b border-edge px-4 py-2.5 transition-colors last:border-b-0 hover:bg-panel"
              >
                <div className="flex min-w-0 flex-[2] flex-col gap-0.5">
                  <p className="truncate text-[13px] text-ink">{c.name}</p>
                  <span className="flex items-baseline gap-2">
                    <Legend>{KIND_LABEL[c.kind]}</Legend>
                    {c.priority !== 0 && <Legend>priority {c.priority}</Legend>}
                  </span>
                </div>

                <p
                  data-selectable
                  className="describer hidden min-w-0 flex-[3] truncate text-[11px] text-ink-dim sm:block"
                >
                  {c.pattern}
                </p>

                <span className="hidden min-w-0 flex-[3] items-center gap-1.5 text-ink-dim md:flex">
                  <FolderOpen className="size-3.5 shrink-0" />
                  <span data-selectable className="describer truncate text-[11px]">
                    {c.directory}
                  </span>
                </span>

                <div className="flex w-[104px] shrink-0 items-center justify-end gap-2">
                  <Switch
                    label={`${c.enabled ? "Disable" : "Enable"} the ${c.name} rule`}
                    checked={c.enabled}
                    onChange={(enabled) => void categories.setEnabled(c.id, enabled)}
                  />
                  <button
                    type="button"
                    aria-label={`Delete the ${c.name} rule`}
                    title="Delete rule"
                    onClick={() => void categories.remove(c.id)}
                    className="focus-ring flex size-7 items-center justify-center rounded-hw border border-transparent text-ink-dim transition-colors hover:border-danger/45 hover:text-danger"
                  >
                    <Trash className="size-3.5" />
                  </button>
                </div>
              </div>
            ))}
          </div>
        )}
      </Section>
    </div>
  );
}
