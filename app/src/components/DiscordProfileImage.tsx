import { useEffect, useId, useRef, useState } from "react";
import { imageCrop } from "../lib/discordProfile";
import { Button, Field } from "./Controls";

export function DiscordProfileImage({ label, banner = false, value, disabled, onChange, onError, onEditing }: {
  label: string; banner?: boolean; value: string | null; disabled: boolean;
  onChange: (value: string | null) => void; onError: (message: string) => void; onEditing: (editing: boolean) => void;
}) {
  const id = useId();
  const input = useRef<HTMLInputElement>(null);
  const selection = useRef(0);
  useEffect(() => () => { selection.current++; }, []);
  const [image, setImage] = useState<HTMLImageElement | null>(null);
  const [position, setPosition] = useState({ zoom: 1, x: 50, y: 50 });
  const [processing, setProcessing] = useState(false);
  const choose = async (file: File) => {
    const ticket = ++selection.current;
    onError("");
    if (!["image/png", "image/jpeg", "image/webp"].includes(file.type) || !file.size || file.size > 5 * 1024 * 1024) {
      onError("Choose a PNG, JPEG, or WebP image up to 5 MiB."); return;
    }
    setImage(null); setProcessing(true); onEditing(true);
    try {
      const source = await new Promise<string>((resolve, reject) => {
        const reader = new FileReader();
        reader.onload = () => resolve(String(reader.result));
        reader.onerror = () => reject(new Error("Could not read this image. Choose another file."));
        reader.readAsDataURL(file);
      });
      const next = new Image(); next.src = source;
      await next.decode();
      if (ticket !== selection.current) return;
      if (next.width * next.height > 40_000_000 || next.width > 16384 || next.height > 16384) throw new Error("Choose an image smaller than 40 megapixels and 16,384 pixels on each side.");
      setImage(next); setPosition({ zoom: 1, x: 50, y: 50 });
    } catch (e) {
      if (ticket === selection.current) { onError(e instanceof Error ? e.message : "Could not read this image. Choose another file."); onEditing(false); }
    } finally {
      if (ticket === selection.current) setProcessing(false);
    }
  };
  const crop = () => {
    if (!image) return;
    try {
      const canvas = document.createElement("canvas");
      canvas.width = banner ? 1200 : 512; canvas.height = banner ? 480 : 512;
      const context = canvas.getContext("2d");
      if (!context) throw new Error("Image cropping is unavailable. Try reopening the app.");
      const source = imageCrop(image.width, image.height, canvas.width / canvas.height, position.zoom, position.x, position.y);
      context.drawImage(image, source.x, source.y, source.width, source.height, 0, 0, canvas.width, canvas.height);
      onChange(canvas.toDataURL("image/png"));
      setImage(null); onEditing(false);
    } catch (e) { onError(e instanceof Error ? e.message : String(e)); }
  };
  return <Field label={label} hint={`${banner ? "Wide crop, 1200 × 480" : "Square crop, 512 × 512"}. PNG, JPEG, or WebP up to 5 MiB. Changes apply when you save this profile.`}>
    <input ref={input} type="file" accept="image/png,image/jpeg,image/webp" aria-label={`Upload ${label.toLowerCase()}`} disabled={disabled || processing}
      className="focus-ring w-full min-w-0 text-[12px] text-ink-dim file:mr-3 file:border file:border-edge-strong file:bg-recess file:px-3 file:py-2 file:text-ink"
      onChange={event => { const file = event.target.files?.[0]; event.target.value = ""; if (file) void choose(file); }} />
    {processing && <p role="status" className="text-[12px] text-ink-dim">Reading image…</p>}
    {image && <div className="space-y-3">
      <div className={`overflow-hidden border border-edge bg-recess ${banner ? "aspect-[5/2]" : "aspect-square max-w-48"}`}>
        <canvas ref={canvas => {
          if (!canvas) return;
          canvas.width = banner ? 600 : 256; canvas.height = banner ? 240 : 256;
          const source = imageCrop(image.width, image.height, canvas.width / canvas.height, position.zoom, position.x, position.y);
          canvas.getContext("2d")?.drawImage(image, source.x, source.y, source.width, source.height, 0, 0, canvas.width, canvas.height);
        }} role="img" aria-label={`${label} crop preview`} className="h-full w-full" />
      </div>
      {(["zoom", "x", "y"] as const).map(axis => <label key={axis} htmlFor={`${id}-${axis}`} className="flex items-center gap-3 text-[12px] text-ink-dim">
        <span className="w-24 shrink-0">{axis === "zoom" ? "Zoom" : axis === "x" ? "Horizontal" : "Vertical"}</span>
        <input id={`${id}-${axis}`} type="range" min={axis === "zoom" ? 1 : 0} max={axis === "zoom" ? 3 : 100} step={axis === "zoom" ? 0.05 : 1}
          value={position[axis]} disabled={disabled} className="focus-ring min-w-0 flex-1 accent-route"
          onChange={e => setPosition(p => ({ ...p, [axis]: Number(e.target.value) }))} />
      </label>)}
      <div className="flex flex-wrap gap-2">
        <Button disabled={disabled} onClick={crop}>Use crop</Button>
        <Button tone="ghost" disabled={disabled} onClick={() => { setImage(null); onEditing(false); }}>Cancel crop</Button>
      </div>
    </div>}
    {value && <Button tone="ghost" disabled={disabled || processing || !!image} className="self-start" onClick={() => onChange(null)}>Remove {label.toLowerCase()}</Button>}
  </Field>;
}
